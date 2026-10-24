//! Actual compact Mixer layout and controls, without a native device or display.
use super::*;
use egui::accesskit::Node;

fn frame(ctx: &egui::Context, app: &mut CitrusApp, events: Vec<egui::Event>) -> Vec<Node> {
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 600.0))),
            events,
            ..Default::default()
        },
        |ui| {
            ui.set_max_height(180.0);
            app.mixer(ui);
        },
    );
    output
        .platform_output
        .accesskit_update
        .unwrap()
        .nodes
        .into_iter()
        .map(|(_, node)| node)
        .collect()
}

#[test]
fn compact_mixer_eight_strips_and_fx_remain_reachable() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = CitrusApp::new_boxed_with_services(&cc, false);
    app.workspace.maximized = false;
    app.project.mixer_tracks.truncate(8);
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = frame(&ctx, &mut app, Vec::new());
    }
    for label in ["Pan", "Volume", "M", "S"] {
        let controls: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.label()
                    .is_some_and(|text| text == label || text.ends_with(&format!("· {label}")))
            })
            .collect();
        assert_eq!(controls.len(), 8, "eight {label} controls: {nodes:?}");
        for node in controls {
            let bounds = node.bounds().unwrap();
            assert!(
                bounds.x0 >= 0.0 && bounds.x1 <= 400.0 && bounds.y0 >= 0.0 && bounds.y1 <= 180.0,
                "{label} clipped: {bounds:?}"
            );
        }
    }
    for (label, solo) in [("M", false), ("S", true)] {
        let bounds = nodes
            .iter()
            .filter(|node| node.label() == Some(label))
            .min_by(|a, b| a.bounds().unwrap().x0.total_cmp(&b.bounds().unwrap().x0))
            .unwrap()
            .bounds()
            .unwrap();
        let pos = Pos2::new(
            (bounds.x0 + bounds.x1) as f32 * 0.5,
            (bounds.y0 + bounds.y1) as f32 * 0.5,
        );
        for pressed in [true, false] {
            nodes = frame(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(if solo {
            app.project.mixer_tracks[0].solo
        } else {
            app.project.mixer_tracks[0].muted
        });
    }
    let fx = nodes
        .iter()
        .find(|node| node.label() == Some("FX / ROUTING"))
        .unwrap()
        .bounds()
        .unwrap();
    let pos = Pos2::new((fx.x0 + fx.x1) as f32 * 0.5, (fx.y0 + fx.y1) as f32 * 0.5);
    for pressed in [true, false] {
        let _ = frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    nodes = frame(&ctx, &mut app, Vec::new());
    assert!(
        nodes
            .iter()
            .filter_map(Node::label)
            .any(|label| label.contains("Empty slot")),
        "all existing insert actions remain in the popup"
    );
}

#[test]
fn compact_mixer_fader_retains_zero_unity_and_bounds() {
    let ctx = egui::Context::default();
    let mut value = 0.5;
    let mut rect = Rect::NOTHING;
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        rect = compact_mixer_fader(ui, &mut value, 42.0, false).rect;
    });
    assert_eq!(rect.size(), Vec2::new(22.0, 42.0));
    for (y, expected) in [(rect.top(), 1.0), (rect.bottom() - 0.1, 0.0)] {
        let pos = Pos2::new(rect.center().x, y);
        for pressed in [true, false] {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ui| {
                    compact_mixer_fader(ui, &mut value, 42.0, false);
                },
            );
        }
        assert_eq!(value, expected);
    }
}
