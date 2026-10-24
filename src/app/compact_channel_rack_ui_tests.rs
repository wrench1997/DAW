//! Real rack controls must fit the narrow floating window, not just a mockup.
use super::*;
use egui::accesskit::Node;

fn frame(
    ctx: &egui::Context,
    app: &mut CitrusApp,
    width: f32,
    events: Vec<egui::Event>,
) -> Vec<Node> {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 500.0))),
            events,
            ..Default::default()
        },
        |ui| app.channel_rack(ui),
    )
    .platform_output
    .accesskit_update
    .unwrap()
    .nodes
    .into_iter()
    .map(|(_, node)| node)
    .collect()
}

fn click(ctx: &egui::Context, app: &mut CitrusApp, nodes: &[Node], label: &str) -> Vec<Node> {
    let bounds = nodes
        .iter()
        .find(|node| node.label() == Some(label))
        .unwrap_or_else(|| panic!("missing {label}"))
        .bounds()
        .unwrap();
    let pos = Pos2::new(
        (bounds.x0 + bounds.x1) as f32 / 2.0,
        (bounds.y0 + bounds.y1) as f32 / 2.0,
    );
    let mut result = Vec::new();
    for pressed in [true, false] {
        result = frame(
            ctx,
            app,
            400.0,
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
    result
}

#[test]
fn compact_rack_all_sixteen_steps_and_controls_fit() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = CitrusApp::new_boxed_with_services(&cc, false);
    app.workspace.maximized = false;
    app.project.channels[0].name =
        "An intentionally very long channel name that must truncate".into();
    for width in [400.0, 440.0, 800.0] {
        let mut nodes = Vec::new();
        for _ in 0..3 {
            nodes = frame(&ctx, &mut app, width, Vec::new());
        }
        for channel in &app.project.channels {
            let mut labels: Vec<_> = (1..=16)
                .map(|step| format!("{} step {step}", channel.name))
                .collect();
            labels.extend([
                format!("Mute {}", channel.name),
                format!("Solo {}", channel.name),
                automation_target_label(&AutomationTarget::ChannelPan {
                    channel: channel.id,
                }),
                automation_target_label(&AutomationTarget::ChannelVolume {
                    channel: channel.id,
                }),
                format!("Mixer destination {}", channel.name),
            ]);
            for label in labels {
                let bounds = nodes
                    .iter()
                    .find(|node| node.label() == Some(&label))
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .bounds()
                    .unwrap();
                assert!(
                    bounds.x0 >= 0.0 && bounds.x1 <= f64::from(width) && bounds.y1 <= 500.0,
                    "{label} is clipped at {width}px: {bounds:?}"
                );
                if label.contains(" step ") {
                    assert!(bounds.width() >= 8.0, "step too small: {bounds:?}");
                }
            }
        }
    }
}

#[test]
fn compact_rack_step_mute_solo_and_channel_selection_are_live() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = CitrusApp::new_boxed_with_services(&cc, false);
    app.workspace.maximized = false;
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes = frame(&ctx, &mut app, 400.0, Vec::new());
    }
    let name = app.project.channels[0].name.clone();
    for step in [0, 15] {
        let before = app.project.active_pattern().channel_steps[0][step];
        nodes = click(&ctx, &mut app, &nodes, &format!("{name} step {}", step + 1));
        assert_eq!(app.project.active_pattern().channel_steps[0][step], !before);
    }
    let muted = app.project.channels[0].muted;
    nodes = click(&ctx, &mut app, &nodes, &format!("Mute {name}"));
    assert_eq!(app.project.channels[0].muted, !muted);
    let solo = app.project.channels[0].solo;
    nodes = click(&ctx, &mut app, &nodes, &format!("Solo {name}"));
    assert_eq!(app.project.channels[0].solo, !solo);
    let name = app.project.channels[1].name.clone();
    let _ = click(&ctx, &mut app, &nodes, &name);
    assert_eq!(app.selected_channel, 1);
}
