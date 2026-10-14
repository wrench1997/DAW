use super::*;
use crate::{
    model::{ClipKind, PianoNote, Project},
    tempo_map::TempoMap,
    timeline::TimelineCompileOptions,
};

fn geometry(height: f32, viewport_height: f32, origin: f64) -> PianoKeyboardGeometry {
    let rect = Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(72.0, viewport_height));
    let mut axis = AxisViewport::new(
        0.0,
        128.0,
        f64::from(viewport_height),
        f64::from(height),
        4.0,
        40.0,
    )
    .unwrap();
    axis.scroll_by_pixels(origin * f64::from(height)).unwrap();
    PianoKeyboardGeometry {
        rect,
        pitch_axis: axis,
    }
}

#[test]
fn black_keys_are_shorter_narrower_and_centered_on_their_exact_midi_rows() {
    for height in [6.0, 14.0, 24.0, 32.0] {
        let keyboard = geometry(height, height * 128.0, 0.0);
        for pitch in 0..128_u8 {
            let key = keyboard.key(pitch);
            let expected_center = keyboard.rect.top()
                + keyboard
                    .pitch_axis
                    .pixel_for_content(f64::from(127 - pitch) + 0.5) as f32;
            assert!((key.pitch_row.center().y - expected_center).abs() < 0.001);
            if key.black {
                assert!((key.face.center().y - key.pitch_row.center().y).abs() < 0.001);
                assert!(key.face.height() < key.pitch_row.height());
                assert!(key.face.width() < keyboard.rect.width() * 0.65);
                assert_eq!(keyboard.pitch_at(key.face.center()), Some(pitch));
            } else {
                assert_eq!(key.face.width(), keyboard.rect.width());
                assert_eq!(
                    keyboard.pitch_at(Pos2::new(key.face.right() - 4.0, key.pitch_row.center().y)),
                    Some(pitch)
                );
            }
        }
    }
}

#[test]
fn white_key_underlays_join_at_black_centers_and_have_no_dead_row_gaps() {
    let keyboard = geometry(20.0, 300.0, 60.0);
    let c = keyboard.key(60);
    let cs = keyboard.key(61);
    let d = keyboard.key(62);
    assert_eq!(c.face.top(), cs.pitch_row.center().y);
    assert_eq!(d.face.bottom(), cs.pitch_row.center().y);
    let behind_black = Pos2::new(keyboard.rect.right() - 4.0, cs.pitch_row.center().y - 2.0);
    assert_eq!(keyboard.pitch_at(behind_black), Some(62));
    let below_black = Pos2::new(keyboard.rect.right() - 4.0, cs.pitch_row.center().y + 2.0);
    assert_eq!(keyboard.pitch_at(below_black), Some(60));
    for offset in 0..300 {
        assert!(
            keyboard
                .pitch_at(Pos2::new(
                    keyboard.rect.right() - 4.0,
                    keyboard.rect.top() + offset as f32 + 0.25
                ))
                .is_some()
        );
    }
}

#[test]
fn partial_viewports_and_midi_endpoints_keep_valid_hits_only() {
    for origin in [0.0, 60.3, 112.75] {
        let keyboard = geometry(20.0, 300.0, origin);
        assert!(
            keyboard
                .visible_keys()
                .all(|key| key.face.intersects(keyboard.rect))
        );
        for offset in 0..300 {
            assert!(
                keyboard
                    .pitch_at(Pos2::new(
                        keyboard.rect.left() + 5.0,
                        keyboard.rect.top() + offset as f32 + 0.25
                    ))
                    .is_some()
            );
        }
        assert_eq!(
            keyboard.pitch_at(keyboard.rect.left_top() - Vec2::splat(0.1)),
            None
        );
        assert_eq!(
            keyboard.pitch_at(Pos2::new(f32::NAN, keyboard.rect.center().y)),
            None
        );
    }
    let keyboard = geometry(20.0, 20.0 * 128.0, 0.0);
    for pitch in [0, 127] {
        let key = keyboard.key(pitch);
        assert!(keyboard.rect.contains_rect(key.face));
        assert_eq!(keyboard.pitch_at(key.pitch_row.center()), Some(pitch));
    }
}

struct PointerHarness {
    ctx: egui::Context,
    geometry: PianoKeyboardGeometry,
    time: f64,
}

impl PointerHarness {
    fn new() -> Self {
        let mut harness = Self {
            ctx: egui::Context::default(),
            geometry: geometry(20.0, 300.0, 60.0),
            time: 0.0,
        };
        for _ in 0..3 {
            harness.frame(vec![], true, true, [false; 128]);
        }
        harness
    }

    fn frame(
        &mut self,
        events: Vec<egui::Event>,
        focused: bool,
        enabled: bool,
        playing: [bool; 128],
    ) -> (Option<u8>, egui::FullOutput) {
        self.time += 0.1;
        let mut pressed = None;
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 420.0))),
                time: Some(self.time),
                focused,
                events,
                ..Default::default()
            },
            |ui| {
                let keyboard = show(ui, self.geometry, enabled, &playing, Some(0));
                pressed = keyboard.pressed_pitch;
            },
        );
        (pressed, output)
    }

    fn pointer(&mut self, pos: Pos2, down: Option<bool>) -> (Option<u8>, egui::FullOutput) {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = down {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        self.frame(events, true, true, [false; 128])
    }
}

fn output_contains_fill(output: &egui::FullOutput, fill: Color32) -> bool {
    fn contains(shape: &egui::Shape, fill: Color32) -> bool {
        match shape {
            egui::Shape::Rect(rect) => rect.fill == fill,
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, fill)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| contains(&shape.shape, fill))
}

#[test]
fn real_primary_press_has_no_drag_threshold_and_release_restores_the_raised_face() {
    for pitch in [60, 61] {
        let mut ui = PointerHarness::new();
        let point = ui.geometry.key(pitch).face.center();
        let (hover, hovered) = ui.pointer(point, None);
        assert_eq!(hover, None);
        let pressed_fill = face_color(
            is_black(pitch),
            KeyState {
                pressed: true,
                ..Default::default()
            },
        );
        assert!(!output_contains_fill(&hovered, pressed_fill));
        let (pressed, output) = ui.pointer(point, Some(true));
        assert_eq!(
            pressed,
            Some(pitch),
            "must light on pointer-down before drag threshold"
        );
        assert!(output_contains_fill(&output, pressed_fill));
        let (held, _) = ui.pointer(point, None);
        assert_eq!(held, Some(pitch));
        let (released, output) = ui.pointer(point, Some(false));
        assert_eq!(released, None);
        assert!(!output_contains_fill(&output, pressed_fill));
    }
}

#[test]
fn held_pointer_crosses_black_and_white_faces_without_sticking_when_it_leaves() {
    let mut ui = PointerHarness::new();
    let c = ui.geometry.key(60).face.center();
    let cs = ui.geometry.key(61).face.center();
    assert_eq!(ui.pointer(c, Some(true)).0, Some(60));
    assert_eq!(ui.pointer(cs, None).0, Some(61));
    let behind = Pos2::new(ui.geometry.rect.right() - 4.0, cs.y - 2.0);
    assert_eq!(ui.pointer(behind, None).0, Some(62));
    assert_eq!(
        ui.pointer(ui.geometry.rect.right_bottom() + Vec2::splat(10.0), None)
            .0,
        None
    );
    assert_eq!(ui.pointer(cs, Some(false)).0, None);
}

#[test]
fn focus_loss_and_app_cancellation_clear_the_depression_without_waiting_for_release() {
    for cancellation in [false, true] {
        let mut ui = PointerHarness::new();
        let point = ui.geometry.key(61).face.center();
        assert_eq!(ui.pointer(point, Some(true)).0, Some(61));
        let (pressed, output) = ui.frame(vec![], cancellation, !cancellation, [false; 128]);
        assert_eq!(pressed, None);
        assert!(!output_contains_fill(
            &output,
            face_color(
                true,
                KeyState {
                    pressed: true,
                    ..Default::default()
                }
            )
        ));
        assert_eq!(
            ui.frame(vec![], true, true, [false; 128]).0,
            None,
            "re-enabling must not resume the cancelled held gesture"
        );
        assert_eq!(ui.pointer(point, Some(false)).0, None);
        assert_eq!(ui.pointer(point, Some(true)).0, Some(61));
    }
}

#[test]
fn external_drag_and_secondary_click_never_depress_a_key() {
    let mut ui = PointerHarness::new();
    let point = ui.geometry.key(61).face.center();
    assert_eq!(ui.pointer(Pos2::new(220.0, 200.0), Some(true)).0, None);
    assert_eq!(ui.pointer(point, None).0, None);
    ui.pointer(point, Some(false));
    let (pressed, _) = ui.frame(
        vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
        true,
        true,
        [false; 128],
    );
    assert_eq!(pressed, None);
}

#[test]
fn playback_face_is_distinct_from_hover_pressed_and_default_octave_faces() {
    let mut ui = PointerHarness::new();
    let mut activity = [false; 128];
    activity[60] = true;
    activity[61] = true;
    let (_, playing) = ui.frame(vec![], true, true, activity);
    for black in [false, true] {
        let active_fill = face_color(
            black,
            KeyState {
                playing: true,
                ..Default::default()
            },
        );
        assert!(output_contains_fill(&playing, active_fill));
        assert_ne!(
            active_fill,
            face_color(
                black,
                KeyState {
                    pressed: true,
                    ..Default::default()
                }
            )
        );
        assert_ne!(
            active_fill,
            face_color(
                black,
                KeyState {
                    hovered: true,
                    ..Default::default()
                }
            )
        );
        assert_ne!(active_fill, face_color(black, KeyState::default()));
    }
    let (_, stopped) = ui.frame(vec![], true, true, [false; 128]);
    assert!(!output_contains_fill(
        &stopped,
        face_color(
            false,
            KeyState {
                playing: true,
                ..Default::default()
            }
        )
    ));
    assert!(output_contains_fill(
        &stopped,
        face_color(false, KeyState::default())
    ));
}

fn compiled_timeline() -> Arc<CompiledTimeline> {
    let mut project = Project::default();
    for pattern in &mut project.patterns {
        pattern.notes.clear();
        for steps in &mut pattern.channel_steps {
            steps.fill(false);
        }
    }
    let channel_id = project.channels[0].id;
    let pattern_id = project.patterns[0].id;
    project.patterns[0].notes = vec![
        PianoNote {
            id: 10,
            channel_id: Some(channel_id),
            note: 60,
            start: 0.0,
            length: 1.0,
            velocity: 0.7,
            group_id: None,
            selected: false,
            muted: false,
        },
        PianoNote {
            id: 11,
            channel_id: Some(channel_id),
            note: 60,
            start: 0.5,
            length: 1.0,
            velocity: 0.7,
            group_id: None,
            selected: false,
            muted: false,
        },
        PianoNote {
            id: 12,
            channel_id: Some(channel_id),
            note: 61,
            start: 2.0,
            length: 1.0,
            velocity: 0.7,
            group_id: None,
            selected: false,
            muted: false,
        },
    ];
    let clip = &mut project.clips[0];
    clip.kind = ClipKind::Pattern;
    clip.pattern_id = pattern_id;
    clip.start = 0.0;
    clip.length = 4.0;
    clip.source_offset = 0.0;
    project.clips.truncate(1);
    let tempo = TempoMap::from_project(&project, 48_000).unwrap();
    Arc::new(
        CompiledTimeline::from_project(&project, &tempo, TimelineCompileOptions::default())
            .unwrap(),
    )
}

fn position(channel_id: u32, frame: u64) -> KeyboardPlaybackPosition {
    KeyboardPlaybackPosition {
        channel_id,
        transport_epoch: 1,
        seek_serial: 1,
        loop_count: 0,
        frame_exclusive: frame,
        discontinuity_frame: Some(0),
    }
}

#[test]
fn callback_frame_activity_preserves_overlapping_same_pitch_ids_and_exclusive_edges() {
    let timeline = compiled_timeline();
    let channel_id = timeline.channel_bases()[0].channel_id;
    let ons: Vec<_> = timeline
        .events()
        .iter()
        .filter(|event| matches!(event.kind, TimelineEventKind::NoteOn { note: 60, .. }))
        .collect();
    let offs: Vec<_> = timeline
        .events()
        .iter()
        .filter(|event| matches!(event.kind, TimelineEventKind::NoteOff { note: 60, .. }))
        .collect();
    assert_eq!(ons.len(), 2);
    assert_eq!(offs.len(), 2);
    let mut cache = KeyboardPlaybackCache::default();
    assert!(!cache.update(&timeline, position(channel_id, ons[0].frame))[60]);
    assert!(cache.update(&timeline, position(channel_id, ons[0].frame + 1))[60]);
    assert!(cache.update(&timeline, position(channel_id, offs[0].frame + 1))[60]);
    assert!(cache.update(&timeline, position(channel_id, offs[1].frame))[60]);
    assert!(!cache.update(&timeline, position(channel_id, offs[1].frame + 1))[60]);
    let cursor = cache.cursor;
    cache.update(&timeline, position(channel_id, offs[1].frame + 1));
    assert_eq!(
        cache.cursor, cursor,
        "unchanged frame must not replay past events"
    );
}

#[test]
fn seeks_with_no_note_chase_never_light_old_sustains_and_exact_boundary_notes_can_start() {
    let timeline = compiled_timeline();
    let channel_id = timeline.channel_bases()[0].channel_id;
    let first_off = timeline
        .events()
        .iter()
        .find(|event| matches!(event.kind, TimelineEventKind::NoteOff { note: 60, .. }))
        .unwrap()
        .frame;
    let next_on = timeline
        .events()
        .iter()
        .find(|event| matches!(event.kind, TimelineEventKind::NoteOn { note: 61, .. }))
        .unwrap()
        .frame;
    let mut cache = KeyboardPlaybackCache::default();
    assert!(cache.update(&timeline, position(channel_id, 1))[60]);
    let mut seek = position(channel_id, first_off / 2);
    seek.transport_epoch = 2;
    seek.discontinuity_frame = Some(seek.frame_exclusive);
    assert!(!cache.update(&timeline, seek)[60]);
    seek.frame_exclusive = next_on + 1;
    assert!(cache.update(&timeline, seek)[61]);
    let mut exact = position(channel_id, next_on + 1);
    exact.transport_epoch = 3;
    exact.discontinuity_frame = Some(next_on);
    assert!(cache.update(&timeline, exact)[61]);
}

#[test]
fn unconfirmed_restart_channel_changes_backward_frames_and_stop_clear_old_activity() {
    let timeline = compiled_timeline();
    let channel_id = timeline.channel_bases()[0].channel_id;
    let mut cache = KeyboardPlaybackCache::default();
    assert!(cache.update(&timeline, position(channel_id, 1))[60]);
    let mut unknown = position(channel_id, 2);
    unknown.transport_epoch = 2;
    unknown.discontinuity_frame = None;
    assert!(
        !cache
            .update(&timeline, unknown)
            .iter()
            .any(|active| *active)
    );
    let other = position(u32::MAX, 1);
    assert!(!cache.update(&timeline, other).iter().any(|active| *active));
    assert!(cache.update(&timeline, position(channel_id, 1))[60]);
    cache.clear();
    assert!(cache.pitches.iter().all(|active| !active));
    assert!(cache.timeline.is_none());
}

#[test]
fn replacement_arc_and_loop_restart_use_the_new_compiled_source() {
    let timeline = compiled_timeline();
    let channel_id = timeline.channel_bases()[0].channel_id;
    let mut cache = KeyboardPlaybackCache::default();
    assert!(cache.update(&timeline, position(channel_id, 1))[60]);
    let replacement = Arc::new((*timeline).clone());
    assert!(!Arc::ptr_eq(&timeline, &replacement));
    assert!(!cache.update(&replacement, position(channel_id, 0))[60]);
    assert!(cache.update(&replacement, position(channel_id, 1))[60]);
    let mut looped = position(channel_id, 1);
    looped.loop_count = 1;
    looped.discontinuity_frame = Some(0);
    assert!(cache.update(&replacement, looped)[60]);
    assert_eq!(
        cache.active.len(),
        1,
        "a loop must replace the prior held note identity"
    );
}

#[test]
fn a_new_seek_serial_resets_even_if_the_observed_epoch_and_frame_did_not_change() {
    let timeline = compiled_timeline();
    let channel_id = timeline.channel_bases()[0].channel_id;
    let mut cache = KeyboardPlaybackCache::default();
    assert!(cache.update(&timeline, position(channel_id, 1))[60]);
    let mut seek = position(channel_id, 1);
    seek.seek_serial += 1;
    seek.discontinuity_frame = Some(1);
    assert!(!cache.update(&timeline, seek)[60]);
}

fn admission() -> KeyboardPlaybackAdmission {
    KeyboardPlaybackAdmission {
        activation: Some((2, 3, 4, 5, 9)),
        current: (2, 3, 4, 5),
        confirmed_timeline: Some((4, 9)),
        transport_epoch: 9,
        loop_count: 7,
        seek_frame: 1_337,
        loop_start_frame: 64,
    }
}

#[test]
fn fresh_seek_uses_its_accepted_frame_even_after_accumulated_loops() {
    let activation = admission();
    assert!(activation.loop_count > 0);
    assert_eq!(activation.discontinuity_frame(), Some(1_337));
    let mut later_loop = activation;
    later_loop.transport_epoch = 10;
    later_loop.confirmed_timeline = Some((4, 10));
    assert_eq!(later_loop.discontinuity_frame(), Some(64));
    later_loop.loop_count = 0;
    assert_eq!(
        later_loop.discontinuity_frame(),
        None,
        "an unexplained later epoch is not a confirmed loop boundary"
    );
}

#[test]
fn callback_snapshot_epoch_and_confirmed_generation_must_match_exactly() {
    let mut state = admission();
    state.transport_epoch = 10;
    assert_eq!(
        state.discontinuity_frame(),
        None,
        "a snapshot from a newer publication cannot use the older confirmed epoch"
    );
    state.confirmed_timeline = Some((99, 10));
    assert_eq!(state.discontinuity_frame(), None);
    state.confirmed_timeline = None;
    assert_eq!(state.discontinuity_frame(), None);
    state.confirmed_timeline = Some((4, 10));
    assert_eq!(state.discontinuity_frame(), Some(64));
}

#[test]
fn stale_session_generation_seek_and_activation_are_rejected_and_epoch_wrap_is_validated() {
    for current in [(99, 3, 4, 5), (2, 99, 4, 5), (2, 3, 99, 5), (2, 3, 4, 99)] {
        let mut state = admission();
        state.current = current;
        assert_eq!(
            state.discontinuity_frame(),
            None,
            "stale identity {current:?}"
        );
    }
    let mut state = admission();
    state.activation = None;
    assert_eq!(state.discontinuity_frame(), None);
    state.activation = Some((2, 3, 4, 5, 10));
    assert_eq!(
        state.discontinuity_frame(),
        None,
        "an older snapshot cannot reach a future activation"
    );
    state.activation = Some((2, 3, 4, 5, 0));
    assert_eq!(state.discontinuity_frame(), None);
    state.activation = Some((2, 3, 4, 5, u64::MAX));
    state.transport_epoch = 1;
    state.confirmed_timeline = Some((4, 1));
    assert_eq!(
        state.discontinuity_frame(),
        Some(64),
        "a later wrapped loop epoch is valid"
    );
    state.loop_count = 0;
    assert_eq!(state.discontinuity_frame(), None);
    state.transport_epoch = 0;
    state.confirmed_timeline = Some((4, 0));
    assert_eq!(state.discontinuity_frame(), None);
}
