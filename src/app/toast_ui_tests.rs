//! Actual App notification layout after changing message size and viewport.
use super::*;

fn toast_rect(ui: &UiHarness) -> Rect {
    ui.ctx
        .memory(|memory| memory.area_rect(Id::new("toast")))
        .expect("the production toast Area must have layout bounds")
}

fn toast_text<'a>(
    output: &'a egui::FullOutput,
    message: &str,
) -> (&'a egui::epaint::TextShape, Rect) {
    fn text<'a>(shape: &'a egui::Shape, message: &str) -> Option<&'a egui::epaint::TextShape> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == message => Some(text),
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text(shape, message)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| text(&shape.shape, message).map(|text| (text, shape.clip_rect)))
        .expect("the complete notification must be painted by its production Label")
}

fn assert_inside_viewport(ui: &UiHarness) {
    let rect = toast_rect(ui);
    let viewport = ui.ctx.content_rect();
    assert!(
        viewport.expand(1.0).contains_rect(rect),
        "notification must fit its actual viewport: {rect:?} / {viewport:?}"
    );
    assert!(rect.width().is_finite() && rect.height().is_finite());
    assert!(rect.width() > 26.0 && rect.height() > 18.0);
}

fn long_saved_path() -> String {
    format!(
        "Saved D:\\workspaces\\DAW\\target\\melody-stability-20261011\\desktop-phrase-05\\{}\\saved-noire-pure-window-and-undo-checkpoint.citrus",
        "private-melody-project-with-preserved-original-notes-and-native-plugin-state-".repeat(2)
    )
}

#[test]
fn toast_ui_short_then_long_path_fits_on_the_first_and_second_real_frames() {
    let mut ui = UiHarness::new();
    ui.app.notify("Saved".into());
    ui.run(Vec::new());
    ui.run(Vec::new());
    let short = toast_rect(&ui);
    assert!(short.width() < 120.0 && short.height() < 45.0);

    let message = long_saved_path();
    assert!(message.len() > 200);
    ui.app.notify(message.clone());
    for _ in 0..2 {
        let output = ui.run(Vec::new());
        let rect = toast_rect(&ui);
        assert_inside_viewport(&ui);
        assert!(rect.width() > 400.0 && rect.width() <= 527.0);
        assert!(
            rect.height() < 150.0,
            "path must not become a narrow column: {rect:?}"
        );
        let (text, _) = toast_text(&output, &message);
        assert!(!text.galley.elided, "the full saved path must be retained");
        assert!(rect.expand(1.0).contains_rect(text.visual_bounding_rect()));
    }
    ui.capture("toast-short-to-long-path");
}

#[test]
fn toast_ui_long_then_short_returns_to_a_compact_frame() {
    let mut ui = UiHarness::new();
    ui.app.notify(long_saved_path());
    ui.run(Vec::new());
    ui.run(Vec::new());
    assert!(toast_rect(&ui).width() > 400.0);

    ui.app.notify("Saved".into());
    for _ in 0..2 {
        let output = ui.run(Vec::new());
        let rect = toast_rect(&ui);
        assert_inside_viewport(&ui);
        assert!(rect.width() < 120.0 && rect.height() < 45.0, "{rect:?}");
        assert!(
            rect.expand(1.0)
                .contains_rect(toast_text(&output, "Saved").0.visual_bounding_rect())
        );
    }
}

#[test]
fn toast_ui_viewport_resize_and_zoom_keep_long_messages_inside_the_window() {
    let mut ui = UiHarness::new();
    let message = long_saved_path();
    ui.app.notify(message.clone());
    ui.run(Vec::new());
    ui.run(Vec::new());

    for scale in [1.0, 1.5, 2.0] {
        ui.ctx.set_zoom_factor(scale);
        // Simulate the logical space available in a 1080 x 680 physical viewport.
        ui.size = Vec2::new(1080.0, 680.0) / scale;
        for _ in 0..2 {
            let output = ui.run(Vec::new());
            assert_inside_viewport(&ui);
            let rect = toast_rect(&ui);
            assert!(
                rect.width() > 250.0,
                "the message must not reuse a narrow width: {rect:?}"
            );
            assert!(rect.height() < ui.ctx.content_rect().height() * 0.5);
            let (text, _) = toast_text(&output, &message);
            assert!(!text.galley.elided);
            assert!(rect.expand(1.0).contains_rect(text.visual_bounding_rect()));
        }
    }
    ui.capture("toast-minimum-viewport-zoom");
}

#[test]
fn toast_ui_multiline_details_remain_complete_and_scroll_inside_a_bounded_frame() {
    let mut ui = UiHarness::new();
    ui.size = Vec2::new(1080.0, 680.0);
    ui.app.notify("Saved".into());
    ui.run(Vec::new());
    ui.run(Vec::new());
    let message = (0..100)
        .map(|line| format!("Detail {line:03}: native instrument state restore diagnostic\n"))
        .collect::<String>();
    ui.app.notify(message.clone());
    let output = ui.run(Vec::new());
    assert_inside_viewport(&ui);
    let rect = toast_rect(&ui);
    assert!(rect.height() < ui.ctx.content_rect().height());
    let (text, clip) = toast_text(&output, &message);
    assert!(!text.galley.elided);
    assert!(
        text.galley.size().y > rect.height(),
        "details must remain available beyond the visible scroll area"
    );
    assert!(
        rect.expand(1.0).contains_rect(clip),
        "text clip must stay inside the notification frame: {clip:?} / {rect:?}"
    );
    let original_y = text.pos.y;

    ui.run(vec![
        egui::Event::PointerMoved(rect.center()),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            phase: egui::TouchPhase::Move,
            delta: Vec2::new(0.0, -3000.0),
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let mut scrolled = ui.run(Vec::new());
    for _ in 0..10 {
        scrolled = ui.run(Vec::new());
    }
    assert_inside_viewport(&ui);
    let (text, clip) = toast_text(&scrolled, &message);
    assert!(!text.galley.elided);
    assert!(
        text.pos.y < original_y - 20.0,
        "real wheel input must reveal the preserved details"
    );
    assert!(
        toast_rect(&ui).expand(1.0).contains_rect(clip),
        "scrolled text clip must stay inside its frame: {clip:?} / {:?}",
        toast_rect(&ui)
    );
    ui.capture("toast-multiline-scrolled");
}
