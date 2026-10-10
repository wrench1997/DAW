//! Bounded driver-only health evidence for the ignored genuine fixture.
//! No SDK call, callback pump, owner lock, opaque state, or PCM observation.
use super::*;

const TEXT_BYTES: usize = 1024;
const INVENTORY_ITEMS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HealthClass {
    Clear,
    Wait,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HealthStatus {
    pub class: HealthClass,
    phase: TimelineAudioSyncPhase,
    non_phase_pending: bool,
}
impl HealthStatus {
    pub fn admits_condition(self, condition: bool) -> bool {
        self.class == HealthClass::Clear && condition
    }

    // The first exact callback activation precedes App receipt consumption.
    // Only that exact pending phase is exempt; no owner/asset/compile wait is.
    pub fn admits_first_activation(
        self,
        generation: u64,
        actual_epoch: u64,
        observed_legacy_barrier: u64,
        expected_legacy_barrier: u64,
    ) -> bool {
        observed_legacy_barrier == expected_legacy_barrier
            && (self.class == HealthClass::Clear
                || (self.class == HealthClass::Wait
                    && !self.non_phase_pending
                    && matches!(self.phase, TimelineAudioSyncPhase::AwaitingActivation {
                        generation: wanted_generation, epoch: minimum_epoch,
                        frame: 0, activation_request_id, ..
                    } if generation == wanted_generation
                        && timeline_epoch_has_reached(actual_epoch, minimum_epoch)
                        && activation_request_id != 0)))
    }
    pub fn pending_activation(self) -> Option<TimelineAudioSyncPhase> {
        (self.class == HealthClass::Wait
            && matches!(
                self.phase,
                TimelineAudioSyncPhase::AwaitingActivation { .. }
            ))
        .then_some(self.phase)
    }
    pub fn confirms_activation(
        self,
        expected_phase: TimelineAudioSyncPhase,
        actual_epoch: u64,
    ) -> bool {
        self.class == HealthClass::Clear
            && matches!((expected_phase, self.phase),
                (TimelineAudioSyncPhase::AwaitingActivation { generation: expected_generation,
                    epoch: minimum_epoch, frame:0, activation_request_id, .. },
                 TimelineAudioSyncPhase::Active { generation, epoch })
                if activation_request_id != 0 && generation == expected_generation
                    && epoch == actual_epoch && timeline_epoch_has_reached(epoch, minimum_epoch))
    }
}

pub(super) struct HealthObservation {
    pub status: HealthStatus,
    pub evidence: Value,
}

fn bounded_text(text: &str) -> Value {
    let mut end = text.len().min(TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Bound before JSON makes its String copy. Retain exact bytes if in range.
    json!({"text": &text[..end], "original_bytes": text.len(),
        "retained_bytes": end, "truncated": end != text.len()})
}
fn optional_text(text: Option<&str>) -> Value {
    text.map(bounded_text).unwrap_or(Value::Null)
}
fn inventory<I: ExactSizeIterator>(items: I, item: impl Fn(I::Item) -> Value) -> Value {
    let total = items.len();
    let entries = items.take(INVENTORY_ITEMS).map(item).collect::<Vec<_>>();
    json!({"count":total,"truncated":total > INVENTORY_ITEMS,"entries":entries})
}

#[derive(Clone, Copy)]
struct HealthOperands {
    audio_present: bool,
    processing_retry: bool,
    restoration_failed: bool,
    compile_error: bool,
    cap_exceeded: bool,
    phase: TimelineAudioSyncPhase,
    route_failed_authoritative: bool,
    route_pending_authoritative: bool,
    owners_pending: bool,
    assets_pending: bool,
    compile_pending: bool,
}
fn classify(operands: HealthOperands) -> (HealthStatus, Vec<&'static str>) {
    let mut failures = Vec::with_capacity(7);
    for (failed, name) in [
        (!operands.audio_present, "missing_audio_engine"),
        (
            operands.processing_retry,
            "plugin_processing_retry_required",
        ),
        (operands.restoration_failed, "clip_restoration_failed"),
        (operands.compile_error, "timeline_compile_error"),
        (operands.cap_exceeded, "diagnostic_inventory_cap"),
        (
            operands.route_failed_authoritative,
            "current_dispatch_route_failed",
        ),
        (
            matches!(
                operands.phase,
                TimelineAudioSyncPhase::Faulted
                    | TimelineAudioSyncPhase::FaultClearReady { .. }
                    | TimelineAudioSyncPhase::FaultClearQueued { .. }
            ),
            "terminal_timeline_phase",
        ),
    ] {
        if failed {
            failures.push(name);
        }
    }
    let non_phase_pending = operands.route_pending_authoritative
        || operands.owners_pending
        || operands.assets_pending
        || operands.compile_pending;
    let class = if !failures.is_empty() {
        HealthClass::Failed
    } else if non_phase_pending || !matches!(operands.phase, TimelineAudioSyncPhase::Active { .. })
    {
        HealthClass::Wait
    } else {
        HealthClass::Clear
    };
    (
        HealthStatus {
            class,
            phase: operands.phase,
            non_phase_pending,
        },
        failures,
    )
}

fn phase_evidence(phase: TimelineAudioSyncPhase) -> Value {
    use TimelineAudioSyncPhase::*;
    match phase {
        Unavailable => json!({"variant":"Unavailable"}),
        InstallReady => json!({"variant":"InstallReady"}),
        InstallQueued {
            request_id,
            generation,
        } => json!({"variant":"InstallQueued","request_id":request_id,"generation":generation}),
        LoopReady => json!({"variant":"LoopReady"}),
        LoopQueued {
            request_id,
            generation,
            frame,
        } => {
            json!({"variant":"LoopQueued","request_id":request_id,"generation":generation,"frame":frame})
        }
        SeekReady => json!({"variant":"SeekReady"}),
        SeekQueued {
            request_id,
            generation,
            seek_serial,
            epoch,
            frame,
        } => {
            json!({"variant":"SeekQueued","request_id":request_id,"generation":generation,"seek_serial":seek_serial,"epoch":epoch,"frame":frame})
        }
        AwaitingActivation {
            generation,
            seek_serial,
            epoch,
            frame,
            activation_request_id,
        } => {
            json!({"variant":"AwaitingActivation","generation":generation,"seek_serial":seek_serial,"epoch":epoch,"frame":frame,"activation_request_id":activation_request_id})
        }
        Active { generation, epoch } => {
            json!({"variant":"Active","generation":generation,"epoch":epoch})
        }
        AssetHandoffClearReady { revision } => {
            json!({"variant":"AssetHandoffClearReady","revision":revision})
        }
        AssetHandoffClearQueued {
            request_id,
            revision,
        } => {
            json!({"variant":"AssetHandoffClearQueued","request_id":request_id,"revision":revision})
        }
        FaultClearReady { revision } => json!({"variant":"FaultClearReady","revision":revision}),
        FaultClearQueued {
            request_id,
            revision,
        } => json!({"variant":"FaultClearQueued","request_id":request_id,"revision":revision}),
        Faulted => json!({"variant":"Faulted"}),
    }
}
fn operation(operation: AudioAssetOperation) -> Value {
    json!({"engine_session":operation.engine_session,"generation":operation.generation,
        "operation_id":operation.operation_id})
}

fn route_is_authoritative(
    source_current: bool,
    dispatch_allowed: bool,
    insert_replacements_ready: bool,
    phase: TimelineAudioSyncPhase,
) -> bool {
    source_current
        && dispatch_allowed
        && insert_replacements_ready
        && matches!(
            phase,
            TimelineAudioSyncPhase::InstallReady
                | TimelineAudioSyncPhase::LoopReady
                | TimelineAudioSyncPhase::SeekReady
        )
}

pub(super) fn observe(app: &CitrusApp) -> HealthObservation {
    let sync = &app.timeline_audio_sync;
    // Check each inventory length before calling production helpers that collect
    // routes/assets. On overflow, record capped prefixes and fail closed.
    let counts = [
        ("running_generators", app.running_generator_chains.len()),
        ("pending_generators", app.pending_generator_chains.len()),
        ("generator_removals", app.pending_generator_removals.len()),
        ("generator_routes", app.pending_generator_routes.len()),
        ("deferred_generators", app.deferred_generator_rebuilds.len()),
        ("pending_restorations", app.pending_clip_restorations.len()),
        ("pending_inserts", app.pending_insert_chains.len()),
        ("insert_removals", app.pending_insert_removals.len()),
        ("loaded_assets", app.loaded_audio_assets.len()),
        ("loading_assets", app.loading_audio_assets.len()),
        (
            "prepared_assets",
            app.prepared_audio_asset_registrations.len(),
        ),
        (
            "registering_assets",
            app.pending_audio_asset_registrations.len(),
        ),
        (
            "unregistering_assets",
            app.pending_audio_asset_unregistrations.len(),
        ),
        ("failed_assets", app.failed_audio_assets.len()),
        (
            "source_routes",
            app.timeline_audio_source
                .as_ref()
                .map_or(0, |s| s.timeline.plugin_routes().len()),
        ),
        (
            "source_bases",
            app.timeline_audio_source
                .as_ref()
                .map_or(0, |s| s.timeline.channel_bases().len()),
        ),
        (
            "source_audio_clips",
            app.timeline_audio_source
                .as_ref()
                .map_or(0, |s| s.timeline.audio_clips().len()),
        ),
    ];
    let cap_exceeded = counts.iter().any(|(_, count)| *count > INVENTORY_ITEMS)
        || app
            .pending_insert_chains
            .values()
            .take(INVENTORY_ITEMS)
            .any(|chain| chain.instance_ids.len() > INVENTORY_ITEMS);
    let source_current = app.timeline_audio_source.as_ref().is_some_and(|source| {
        source.generation == app.timeline_desired_generation
            && timeline_source_identity_is_current(
                source.generation,
                source.fingerprint,
                source.sample_rate,
                app.timeline_compiled_generation,
                app.timeline_compiled_fingerprint,
                app.timeline_observed_fingerprint,
                app.timeline_sample_rate(),
            )
    });
    let dispatch_allowed = !app.clip_session_retirement_pending()
        && app.clip_render_allows_timeline_dispatch()
        && !app.plugin_processing_retry_required;
    let route_authoritative = route_is_authoritative(
        source_current,
        dispatch_allowed,
        app.pending_insert_chains.is_empty() && app.pending_insert_removals.is_empty(),
        sync.phase,
    );
    let route = if cap_exceeded {
        None
    } else {
        app.timeline_audio_source
            .as_ref()
            .map(|source| app.timeline_generator_routes_state(&source.timeline))
    };
    let route_evidence = match &route {
        None => {
            json!({"state":"Unobserved","reason":if cap_exceeded {"inventory_cap"} else {"no_source"}})
        }
        Some(RequiredGeneratorRouteState::Ready) => json!({"state":"Ready"}),
        Some(RequiredGeneratorRouteState::Pending {
            channel_id,
            plugin_instance_id,
        }) => {
            json!({"state":"Pending","channel_id":channel_id,"plugin_instance_id":plugin_instance_id})
        }
        Some(RequiredGeneratorRouteState::Failed(message)) => {
            json!({"state":"Failed","error":bounded_text(message)})
        }
    };
    let required_assets = if cap_exceeded {
        None
    } else {
        app.timeline_audio_source
            .as_ref()
            .map(|source| timeline_required_audio_asset_ids(&source.timeline))
    };
    let asset_state = required_assets.as_ref().map(|required| {
        required_audio_asset_state(
            required.iter().copied(),
            &app.loaded_audio_assets,
            &app.pending_audio_asset_unregistrations,
            &app.failed_audio_assets,
            app.audio_asset_generation,
            app.pending_audio_asset_clear.is_some(),
        )
    });
    let owners_pending = !app.pending_generator_chains.is_empty()
        || !app.pending_generator_removals.is_empty()
        || !app.pending_generator_routes.is_empty()
        || !app.deferred_generator_rebuilds.is_empty()
        || !app.pending_clip_restorations.is_empty()
        || !app.pending_insert_chains.is_empty()
        || !app.pending_insert_removals.is_empty()
        || app.clip_restoration_playback_pending
        || app.clip_session_retirement_pending();
    let assets_pending = !app.loading_audio_assets.is_empty()
        || !app.prepared_audio_asset_registrations.is_empty()
        || !app.pending_audio_asset_registrations.is_empty()
        || !app.pending_audio_asset_unregistrations.is_empty()
        || app.pending_audio_asset_clear.is_some()
        || sync.asset_handoff_silenced
        || (source_current && matches!(asset_state, Some(RequiredAudioAssetState::Loading(_))));
    let compile_pending = app.timeline_pending_generation.is_some()
        || app.timeline_deferred_job.is_some()
        || !source_current;
    let operands = HealthOperands {
        audio_present: app.audio.is_some(),
        processing_retry: app.plugin_processing_retry_required,
        restoration_failed: app.clip_restoration_failed,
        compile_error: app.timeline_compile_error.is_some(),
        cap_exceeded,
        phase: sync.phase,
        route_failed_authoritative: route_authoritative
            && matches!(route, Some(RequiredGeneratorRouteState::Failed(_))),
        route_pending_authoritative: route_authoritative
            && matches!(route, Some(RequiredGeneratorRouteState::Pending { .. })),
        owners_pending,
        assets_pending,
        compile_pending,
    };
    let (status, failures) = classify(operands);
    let evidence = json!({
        "classification":format!("{:?}",status.class),"failed_operands":failures,
        "operands":{"plugin_processing_retry_required":app.plugin_processing_retry_required,
            "clip_restoration_failed":app.clip_restoration_failed,
            "timeline_compile_error":optional_text(app.timeline_compile_error.as_deref()),
            "timeline_audio_sync_last_error":optional_text(sync.last_error.as_deref())},
        "phase":phase_evidence(sync.phase),"pending":{"owners":owners_pending,"assets":assets_pending,
            "compile":compile_pending,"route":operands.route_pending_authoritative},
        "audio_present":app.audio.is_some(),"project_session":app.project_session,
        "engine_session":app.audio.as_ref().map(AudioEngine::engine_session),
        "audio_project_session":app.audio.as_ref().map(AudioEngine::project_session),
        "confirmed_timeline":app.audio.as_ref().and_then(AudioEngine::confirmed_timeline_identity),
        "synchronized":app.audio.as_ref().map(AudioEngine::timeline_runtime_is_synchronized),
        "needs_resync":app.audio.as_ref().map(AudioEngine::timeline_runtime_needs_resync),
        "compile":{"worker_available":app.timeline_worker_available,"desired":app.timeline_desired_generation,
            "pending":app.timeline_pending_generation,"last_enqueued":app.timeline_last_enqueued_generation,
            "deferred":app.timeline_deferred_job.is_some(),"compiled":app.timeline_compiled_generation,
            "compiled_fingerprint":app.timeline_compiled_fingerprint,"observed_fingerprint":app.timeline_observed_fingerprint,
            "pending_chase":app.timeline_pending_chase.is_some(),"pending_loop_chase":app.timeline_pending_loop_chase.is_some()},
        "sync":{"session":sync.session,"source_generation":sync.source_generation,
            "seek_serial":sync.desired_seek.serial,"seek_frame":sync.desired_seek.frame,"seek_beat_bits":sync.desired_seek.beat.to_bits(),
            "loop_token":sync.loop_token,"loop_disable_request":sync.loop_disable_request,"retry_count":sync.retry_count,
            "stale_confirmations":sync.stale_confirmations,"retain_callback_ownership":sync.retain_callback_ownership,
            "asset_handoff_silenced":sync.asset_handoff_silenced,"asset_handoff_frame":sync.asset_handoff_frame,
            "asset_handoff_assets_released":sync.asset_handoff_assets_released},
        "source":app.timeline_audio_source.as_ref().map(|source|json!({"generation":source.generation,
            "fingerprint":source.fingerprint,"sample_rate":source.sample_rate,
            "routes":inventory(source.timeline.plugin_routes().iter(),|r|json!({"instance_id":r.instance_id,"destination":format!("{:?}",r.destination)})),
            "channel_bases":inventory(source.timeline.channel_bases().iter(),|b|json!({"channel_id":b.channel_id,"mixer_track":b.mixer_track}))})),
        "route":{"source_current":source_current,"dispatch_allowed":dispatch_allowed,"authoritative":route_authoritative,
            "state":route_evidence,"session_retirement":app.clip_session_retirement_pending(),
            "clip_dispatch_allowed":app.clip_render_allows_timeline_dispatch(),
            "clip_phase":app.clip_render_job.as_ref().map(|job|format!("{:?}",job.phase))},
        "owners":{
            "running":inventory(app.running_generator_chains.iter(),|(channel,s)|json!({"channel":channel,"project_session":s.project_session,"endpoint":s.endpoint_id,"instance":s.instance_id,"mixer_track":s.mixer_track})),
            "pending":inventory(app.pending_generator_chains.iter(),|(channel,s)|json!({"channel":channel,"project_session":s.project_session,"endpoint":s.endpoint_id,"instance":s.instance_id,"mixer_track":s.mixer_track})),
            "removal":inventory(app.pending_generator_removals.iter(),|(channel,s)|json!({"channel":channel,"project_session":s.project_session,"endpoint":s.endpoint_id,"instance":s.plugin_instance_id})),
            "route":inventory(app.pending_generator_routes.iter(),|(channel,mixer)|json!({"channel":channel,"mixer_track":mixer})),
            "deferred":inventory(app.deferred_generator_rebuilds.iter(),|channel|json!(channel)),
            "restoration_channels":inventory(app.pending_clip_restorations.keys(),|channel|json!(channel)),
            "restoration_playback_pending":app.clip_restoration_playback_pending,
            "pending_inserts":inventory(app.pending_insert_chains.iter(),|(track,s)|json!({"track":track,"project_session":s.project_session,"endpoint":s.endpoint_id,"instances":inventory(s.instance_ids.iter(),|id|json!(id))})),
            "insert_removals":inventory(app.pending_insert_removals.iter(),|(track,s)|json!({"track":track,"project_session":s.project_session,"endpoint":s.endpoint_id}))},
        "assets":{"generation":app.audio_asset_generation,"engine_session":app.audio_asset_engine_session,
            "required":required_assets,"state":asset_state.map(|s|format!("{s:?}")),
            "loaded":inventory(app.loaded_audio_assets.iter(),|(project,runtime)|json!({"project_id":project,"runtime_id":runtime})),
            "loading":inventory(app.loading_audio_assets.iter(),|(id,generation)|json!({"id":id,"generation":generation})),
            "prepared":inventory(app.prepared_audio_asset_registrations.iter(),|(id,s)|json!({"id":id,"project_id":s.project_asset_id,"generation":s.generation})),
            "registering":inventory(app.pending_audio_asset_registrations.iter(),|(id,s)|json!({"id":id,"project_id":s.prepared.project_asset_id,"generation":s.prepared.generation,"operation":operation(s.operation)})),
            "unregistering":inventory(app.pending_audio_asset_unregistrations.iter(),|(id,s)|json!({"id":id,"operation":operation(s.operation)})),
            "clearing":app.pending_audio_asset_clear.map(|s|json!({"operation":operation(s.operation),"queued":s.queued})),
            "failed":inventory(app.failed_audio_assets.iter(),|(id,s)|json!({"id":id,"asset_id":s.asset_id,"generation":s.generation,"message":bounded_text(&s.message)}))},
        "bounds":{"text_bytes":TEXT_BYTES,"inventory_items":INVENTORY_ITEMS,"cap_exceeded":cap_exceeded,
            "counts":counts.into_iter().map(|(name,count)|json!({"name":name,"count":count})).collect::<Vec<_>>()}
    });
    HealthObservation { status, evidence }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(with_route: bool) -> (Box<CitrusApp>, Arc<CompiledTimeline>) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        drop(callback);
        app.audio = Some(engine);
        app.project = fixture_project();
        if with_route {
            app.project.channels[0].instrument_plugin_instance_id = Some(41);
            // Project-only identity. No factory, source, guard, or plugin load.
            app.project.plugin_instances = vec![PluginInstance {
                id: 41,
                format: crate::model::PluginFormat::Vst3,
                role: PluginRole::Instrument,
                path: "/synthetic/health-only.vst3".into(),
                uid: "health-project-only".into(),
                vendor: "Health fixture".into(),
                name: "Project-only route".into(),
                enabled: true,
                bypass: false,
                wet: 1.0,
                parameters: Default::default(),
                opaque_state: vec![],
                runtime_status: crate::model::PluginRuntimeStatus::Loaded,
                midi_ports: Default::default(),
            }];
        }
        let map = TempoMap::from_project(&app.project, 48_000).unwrap();
        let timeline = Arc::new(
            CompiledTimeline::from_project(&app.project, &map, TimelineCompileOptions::default())
                .unwrap(),
        );
        app.timeline_desired_generation = 7;
        app.timeline_compiled_generation = Some(7);
        app.timeline_observed_fingerprint = 23;
        app.timeline_compiled_fingerprint = Some(23);
        app.timeline_pending_generation = None;
        app.timeline_deferred_job = None;
        app.timeline_compile_error = None;
        app.timeline_audio_source = Some(TimelineAudioSource {
            generation: 7,
            fingerprint: 23,
            sample_rate: 48_000,
            timeline: Arc::clone(&timeline),
            tempo_map: map,
            loop_start_beat: 0.0,
            loop_start_frame: 0,
            loop_end_beat: 4.0,
            loop_end_frame: 96_000,
        });
        app.timeline_audio_sync.accept_source(7, false);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::Active {
            generation: 7,
            epoch: 9,
        };
        (app, timeline)
    }

    #[test]
    fn actual_pending_route_setter_waits_without_claiming_stage_completion() {
        let (mut app, timeline) = app(true);
        let channel = app.project.channels[0].id;
        app.pending_generator_routes.insert(channel, 1);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::InstallReady;
        assert!(!app.ensure_timeline_generator_routes_ready(&timeline));
        assert_eq!(
            app.timeline_audio_sync.phase,
            TimelineAudioSyncPhase::InstallReady
        );
        let observed = observe(&app);
        assert_eq!(observed.status.class, HealthClass::Wait);
        assert!(!observed.status.admits_condition(true));
        assert_eq!(observed.evidence["route"]["state"]["state"], "Pending");
        assert_eq!(observed.evidence["route"]["state"]["channel_id"], channel);
        assert_eq!(
            observed.evidence["route"]["state"]["plugin_instance_id"],
            41
        );
        assert_eq!(
            observed.evidence["operands"]["timeline_audio_sync_last_error"]["text"],
            app.timeline_audio_sync.last_error.as_deref().unwrap()
        );
    }

    #[test]
    fn actual_missing_current_route_and_production_fault_remain_terminal() {
        let (mut app, timeline) = app(true);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::InstallReady;
        let before = observe(&app);
        assert_eq!(before.status.class, HealthClass::Failed);
        assert_eq!(before.evidence["route"]["authoritative"], true);
        assert_eq!(before.evidence["route"]["state"]["state"], "Failed");
        assert!(!app.ensure_timeline_generator_routes_ready(&timeline));
        assert!(matches!(
            app.timeline_audio_sync.phase,
            TimelineAudioSyncPhase::Faulted
                | TimelineAudioSyncPhase::FaultClearReady { .. }
                | TimelineAudioSyncPhase::FaultClearQueued { .. }
        ));
        assert_eq!(observe(&app).status.class, HealthClass::Failed);
    }

    #[test]
    fn retained_stale_or_dispatch_blocked_route_is_diagnostic() {
        let (mut app, _) = app(true);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::InstallReady;
        app.timeline_desired_generation = 8;
        let stale = observe(&app);
        assert_eq!(stale.evidence["route"]["state"]["state"], "Failed");
        assert_eq!(stale.evidence["route"]["authoritative"], false);
        assert_eq!(stale.status.class, HealthClass::Wait);
        app.timeline_desired_generation = 7;
        // Active does not perform the production route admission gate; retained
        // route diagnostics cannot manufacture a new fault in that phase.
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::Active {
            generation: 7,
            epoch: 9,
        };
        for (current, dispatch, inserts) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            assert!(!route_is_authoritative(
                current,
                dispatch,
                inserts,
                TimelineAudioSyncPhase::InstallReady
            ));
        }
        assert!(route_is_authoritative(
            true,
            true,
            true,
            TimelineAudioSyncPhase::InstallReady
        ));
        let retained = observe(&app);
        assert_eq!(retained.evidence["route"]["state"]["state"], "Failed");
        assert_eq!(retained.evidence["route"]["authoritative"], false);
        assert_eq!(retained.status.class, HealthClass::Clear);
    }

    #[test]
    fn each_terminal_phase_fails_without_error_text_even_with_pending_owners() {
        let (mut app, _) = app(false);
        for phase in [
            TimelineAudioSyncPhase::Faulted,
            TimelineAudioSyncPhase::FaultClearReady { revision: 7 },
            TimelineAudioSyncPhase::FaultClearQueued {
                request_id: 11,
                revision: 7,
            },
        ] {
            app.timeline_audio_sync.phase = phase;
            app.timeline_audio_sync.last_error = None;
            app.pending_generator_routes.insert(1, 1);
            let observed = observe(&app);
            assert_eq!(observed.status.class, HealthClass::Failed);
            assert!(!observed.status.admits_condition(true));
            assert_eq!(
                observed.evidence["operands"]["timeline_audio_sync_last_error"],
                Value::Null
            );
        }
    }

    #[test]
    fn independent_flags_are_precise_and_faults_win_over_pending() {
        let (mut app, _) = app(false);
        for bits in 0..16 {
            app.plugin_processing_retry_required = bits & 1 != 0;
            app.clip_restoration_failed = bits & 2 != 0;
            app.timeline_compile_error = (bits & 4 != 0).then(|| "compile operand".into());
            app.timeline_audio_sync.last_error =
                (bits & 8 != 0).then(|| "arbitrary diagnostic".into());
            app.pending_generator_routes.insert(1, 1);
            let observed = observe(&app);
            assert_eq!(
                observed.status.class,
                if bits & 7 == 0 {
                    HealthClass::Wait
                } else {
                    HealthClass::Failed
                }
            );
            assert_eq!(
                observed.evidence["operands"]["plugin_processing_retry_required"],
                bits & 1 != 0
            );
            assert_eq!(
                observed.evidence["operands"]["clip_restoration_failed"],
                bits & 2 != 0
            );
            assert_eq!(
                observed.evidence["operands"]["timeline_compile_error"].is_null(),
                bits & 4 == 0
            );
            assert_eq!(
                observed.evidence["operands"]["timeline_audio_sync_last_error"].is_null(),
                bits & 8 == 0
            );
            assert!(!observed.status.admits_condition(true));
        }
    }

    #[test]
    fn error_storage_alone_does_not_decide_admission() {
        let (mut app, _) = app(false);
        for message in [
            None,
            Some(""),
            Some("fault-looking words are only diagnostic"),
            Some("waiting"),
        ] {
            app.timeline_audio_sync.last_error = message.map(str::to_owned);
            let observed = observe(&app);
            assert_eq!(observed.status.class, HealthClass::Clear);
            assert!(observed.status.admits_condition(true));
            assert!(!observed.status.admits_condition(false));
        }
    }

    #[test]
    fn real_queue_and_asset_handoff_states_wait_but_missing_audio_fails() {
        let (mut app, _) = app(false);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::Unavailable;
        assert_eq!(observe(&app).status.class, HealthClass::Wait);
        app.timeline_audio_sync.accept_source(7, false);
        app.timeline_audio_sync.queue_full();
        assert_eq!(observe(&app).status.class, HealthClass::Wait);
        app.timeline_audio_sync.begin_asset_handoff(7, false, 0);
        assert_eq!(observe(&app).status.class, HealthClass::Wait);
        app.timeline_audio_sync.queue_asset_handoff_clear(7, 11);
        assert_eq!(observe(&app).status.class, HealthClass::Wait);
        app.timeline_audio_sync.phase = TimelineAudioSyncPhase::Active {
            generation: 7,
            epoch: 9,
        };
        app.timeline_audio_sync.asset_handoff_silenced = false;
        app.loading_audio_assets
            .insert(42, app.audio_asset_generation);
        assert_eq!(observe(&app).status.class, HealthClass::Wait);
        app.audio = None;
        assert_eq!(observe(&app).status.class, HealthClass::Failed);
    }

    #[test]
    fn utf8_and_inventory_caps_apply_before_diagnostic_copy() {
        let text = "a".repeat(TEXT_BYTES - 1) + &"🦀".repeat(TEXT_BYTES);
        let bounded = bounded_text(&text);
        let prefix = bounded["text"].as_str().unwrap();
        assert_eq!(prefix.len(), TEXT_BYTES - 1);
        assert!(text.starts_with(prefix));
        assert_eq!(bounded["original_bytes"], text.len());
        assert_eq!(bounded["truncated"], true);
        let (mut app, _) = app(false);
        app.timeline_compile_error = Some(text.clone());
        app.timeline_audio_sync.last_error = Some(text);
        for id in 0..INVENTORY_ITEMS + 1 {
            app.pending_generator_routes.insert(id as u32, 1);
        }
        let observed = observe(&app);
        assert_eq!(observed.status.class, HealthClass::Failed);
        assert_eq!(
            observed.evidence["owners"]["route"]["count"],
            INVENTORY_ITEMS + 1
        );
        assert_eq!(
            observed.evidence["owners"]["route"]["entries"]
                .as_array()
                .unwrap()
                .len(),
            INVENTORY_ITEMS
        );
        assert_eq!(observed.evidence["bounds"]["cap_exceeded"], true);
        assert_eq!(
            observed.evidence["route"]["state"]["reason"],
            "inventory_cap"
        );
        assert!(serde_json::to_vec(&observed.evidence).unwrap().len() < 256 * 1024);
    }

    #[test]
    fn every_startup_chase_queue_phase_stays_wait_even_if_condition_is_true() {
        let (mut app, _) = app(false);
        for phase in [
            TimelineAudioSyncPhase::Unavailable,
            TimelineAudioSyncPhase::InstallReady,
            TimelineAudioSyncPhase::InstallQueued {
                request_id: 11,
                generation: 7,
            },
            TimelineAudioSyncPhase::LoopReady,
            TimelineAudioSyncPhase::LoopQueued {
                request_id: 11,
                generation: 7,
                frame: 0,
            },
            TimelineAudioSyncPhase::SeekReady,
            TimelineAudioSyncPhase::SeekQueued {
                request_id: 11,
                generation: 7,
                seek_serial: 3,
                epoch: 9,
                frame: 0,
            },
            TimelineAudioSyncPhase::AwaitingActivation {
                generation: 7,
                seek_serial: 3,
                epoch: 9,
                frame: 0,
                activation_request_id: 11,
            },
            TimelineAudioSyncPhase::AssetHandoffClearReady { revision: 7 },
            TimelineAudioSyncPhase::AssetHandoffClearQueued {
                request_id: 11,
                revision: 7,
            },
        ] {
            app.timeline_audio_sync.phase = phase;
            let observed = observe(&app);
            assert_eq!(observed.status.class, HealthClass::Wait);
            assert!(!observed.status.admits_condition(true));
        }
    }

    #[test]
    fn first_activation_exception_is_exact_and_next_ack_must_be_clear() {
        let base = HealthOperands {
            audio_present: true,
            processing_retry: false,
            restoration_failed: false,
            compile_error: false,
            cap_exceeded: false,
            phase: TimelineAudioSyncPhase::AwaitingActivation {
                generation: 7,
                seek_serial: 3,
                epoch: 9,
                frame: 0,
                activation_request_id: 11,
            },
            route_failed_authoritative: false,
            route_pending_authoritative: false,
            owners_pending: false,
            assets_pending: false,
            compile_pending: false,
        };
        let first = classify(base).0;
        assert_eq!(first.class, HealthClass::Wait);
        assert!(!first.admits_condition(true));
        // Timeline activation11 and legacy mailbox barrier73 are different
        // domains. The actual epoch10 may exceed requested minimum9.
        assert!(first.admits_first_activation(7, 10, 73, 73));
        assert!(first.admits_first_activation(7, 9, 73, 73));
        for (generation, epoch, barrier) in [(8, 10, 73), (7, 8, 73), (7, 10, 11), (7, 10, 74)] {
            assert!(!first.admits_first_activation(generation, epoch, barrier, 73));
        }
        for operands in [
            HealthOperands {
                owners_pending: true,
                ..base
            },
            HealthOperands {
                assets_pending: true,
                ..base
            },
            HealthOperands {
                compile_pending: true,
                ..base
            },
            HealthOperands {
                processing_retry: true,
                ..base
            },
            HealthOperands {
                route_pending_authoritative: true,
                ..base
            },
        ] {
            assert!(!classify(operands).0.admits_first_activation(7, 10, 73, 73));
        }
        let mut sync = TimelineAudioSyncState::new(true);
        sync.accept_source(7, false);
        sync.phase = base.phase;
        // The existing App consumes the exact activation receipt on the next
        // scheduled poll. Model that actual state method without an audio pump.
        sync.desired_seek.serial = 3;
        assert!(!sync.confirm_activation_receipt(73, 7, 10, 0));
        assert_eq!(sync.phase, base.phase);
        assert!(sync.confirm_activation_receipt(11, 7, 10, 0));
        let acknowledged = classify(HealthOperands {
            phase: sync.phase,
            ..base
        })
        .0;
        assert_eq!(acknowledged.class, HealthClass::Clear);
        assert!(acknowledged.admits_condition(true));
        assert!(acknowledged.confirms_activation(base.phase, 10));
        assert!(!acknowledged.confirms_activation(base.phase, 9));
    }
}
