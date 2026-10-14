//! The actual App keyboard widget and its real pointer input; no audio device,
//! plug-in editor, or claim of audible MIDI acceptance.
use super::*;
use crate::app::piano_keyboard::PianoKeyboardGeometry;

fn fixture() -> UiHarness {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.workspace.maximized = true;
    ui.settle();
    ui
}

fn keyboard(ui: &UiHarness) -> PianoKeyboardGeometry {
    PianoKeyboardGeometry {
        rect: ui
            .ctx
            .read_response(Id::new("piano-keyboard"))
            .expect("production keyboard must be registered")
            .rect,
        pitch_axis: ui.app.piano_viewport.y,
    }
}

fn pointer(ui: &mut UiHarness, point: Pos2, pressed: Option<bool>) -> egui::FullOutput {
    let mut events = vec![egui::Event::PointerMoved(point)];
    if let Some(pressed) = pressed {
        events.push(egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    ui.run(events)
}

fn face_fill(output: &egui::FullOutput, face: Rect) -> Color32 {
    fn from_shape(shape: &egui::Shape, face: Rect) -> Option<Color32> {
        match shape {
            egui::Shape::Rect(rect) if rect.rect == face && rect.fill != Color32::TRANSPARENT => {
                Some(rect.fill)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| from_shape(shape, face)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| from_shape(&shape.shape, face))
        .expect("production key face must be painted")
}

fn top_bevel_color(output: &egui::FullOutput, face: Rect) -> Color32 {
    let points = [
        face.left_top() + Vec2::splat(1.0),
        face.right_top() + Vec2::new(-1.0, 1.0),
    ];
    fn from_shape(shape: &egui::Shape, points: [Pos2; 2]) -> Option<Color32> {
        match shape {
            egui::Shape::LineSegment {
                points: actual,
                stroke,
            } if *actual == points => Some(stroke.color),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| from_shape(shape, points)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| from_shape(&shape.shape, points))
        .expect("production key's raised/inset bevel must be painted")
}

fn assert_disabled_color_preserves_the_neutral_face(actual: Color32, neutral: Color32) {
    assert!(actual.a() > 0 && actual.a() < neutral.a());
    // A disabled parent fades every component of the premultiplied color.
    // Alpha rounding can change a reconstructed RGB component by one unit.
    let opacity = f32::from(actual.a()) / f32::from(neutral.a());
    let expected = neutral.gamma_multiply(opacity);
    for (actual, expected) in actual.to_array().into_iter().zip(expected.to_array()) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "disabled color must be the faded neutral face, not the depressed face"
        );
    }
}

#[test]
fn piano_keyboard_ui_real_press_release_has_distinct_faces_without_editing_the_score() {
    let mut ui = fixture();
    let before = project_fingerprint(&ui.app.project);
    let original_history = (ui.app.undo_stack.len(), ui.app.redo_stack.len());
    let geometry = keyboard(&ui);
    let black = geometry.key(61);
    let white = geometry.key(60);
    assert!(black.face.width() < white.face.width() * 0.65);
    assert!(black.face.height() < white.pitch_row.height());
    let rest = ui.run(vec![]);
    let rest_black = face_fill(&rest, black.face);
    let rest_white = face_fill(&rest, white.face);
    ui.capture("piano-keyboard-raised");
    let point = black.face.center();
    let hover = pointer(&mut ui, point, None);
    let hover_black = face_fill(&hover, black.face);
    let pressed = pointer(&mut ui, point, Some(true));
    let pressed_black = face_fill(&pressed, black.face);
    assert_ne!(
        pressed_black, hover_black,
        "hover cannot impersonate a held key"
    );
    assert_ne!(pressed_black, rest_black);
    assert_eq!(
        face_fill(&pressed, white.face),
        rest_white,
        "a black press cannot depress its white underlay"
    );
    ui.capture("piano-keyboard-black-pressed");
    let released = pointer(&mut ui, point, Some(false));
    assert_eq!(face_fill(&released, black.face), hover_black);
    pointer(
        &mut ui,
        geometry.rect.right_bottom() + Vec2::splat(6.0),
        None,
    );
    ui.capture("piano-keyboard-released");
    let white_point = Pos2::new(white.face.right() - 5.0, white.pitch_row.center().y);
    let pressed_white = pointer(&mut ui, white_point, Some(true));
    assert_ne!(face_fill(&pressed_white, white.face), rest_white);
    assert_eq!(face_fill(&pressed_white, black.face), rest_black);
    ui.capture("piano-keyboard-white-pressed");
    pointer(&mut ui, white_point, Some(false));
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(
        (ui.app.undo_stack.len(), ui.app.redo_stack.len()),
        original_history
    );
    assert!(!ui.app.dirty);
    assert!(ui.app.audio.is_none());
}

#[test]
fn piano_keyboard_ui_short_black_key_has_a_playable_white_extension_and_cancelled_hold_does_not_resume()
 {
    let mut ui = fixture();
    let geometry = keyboard(&ui);
    let black = geometry.key(61);
    let white = geometry.key(62);
    let point = Pos2::new(
        geometry.rect.right() - 5.0,
        black.pitch_row.center().y - 1.0,
    );
    assert_eq!(geometry.pitch_at(point), Some(62));
    let rest = ui.run(vec![]);
    let rest_black = face_fill(&rest, black.face);
    let rest_white = face_fill(&rest, white.face);
    let raised_bevel = top_bevel_color(&rest, white.face);
    let pressed = pointer(&mut ui, point, Some(true));
    assert_ne!(face_fill(&pressed, white.face), rest_white);
    assert_ne!(top_bevel_color(&pressed, white.face), raised_bevel);
    assert_eq!(face_fill(&pressed, black.face), rest_black);
    // A pending save is a real App barrier, not a substitute keyboard widget.
    ui.app.queued_save_request = Some(ProjectSaveRequest::Manual {
        path: PathBuf::from("unused-keyboard-barrier.citrus"),
        lifecycle: None,
    });
    let disabled = ui.run(vec![]);
    assert_disabled_color_preserves_the_neutral_face(face_fill(&disabled, white.face), rest_white);
    assert_disabled_color_preserves_the_neutral_face(
        top_bevel_color(&disabled, white.face),
        raised_bevel,
    );
    ui.app.queued_save_request = None;
    let reenabled = ui.run(vec![]);
    assert_eq!(
        face_fill(&reenabled, white.face),
        rest_white,
        "an interrupted hold must wait for a new press"
    );
    assert_eq!(
        top_bevel_color(&reenabled, white.face),
        raised_bevel,
        "re-enabling must restore the raised bevel until a new press"
    );
    pointer(&mut ui, point, Some(false));
    let new_press = pointer(&mut ui, point, Some(true));
    assert_ne!(face_fill(&new_press, white.face), rest_white);
    pointer(&mut ui, point, Some(false));
    assert!(ui.app.audio.is_none());
}
