use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudioIcon {
    Menu,
    Undo,
    Redo,
    Folder,
    Save,
    Play,
    Pause,
    Stop,
    Record,
    Playlist,
    Rack,
    Piano,
    Mixer,
    Plugin,
    Search,
    Pointer,
    Pencil,
    Paint,
    Eraser,
    Slice,
    Slip,
    Fade,
    Mute,
    Group,
    Stamp,
    Magnet,
    Browser,
    Inspector,
    Settings,
    More,
}

pub fn icon_button(ui: &mut Ui, icon: StudioIcon, active: bool, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let fill = if active {
        theme::ORANGE_DIM
    } else if response.hovered() {
        theme::HIGHLIGHT
    } else {
        theme::PANEL_ALT
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    if active || response.has_focus() {
        ui.painter().line_segment(
            [
                rect.left_bottom() + Vec2::new(4.0, -1.0),
                rect.right_bottom() + Vec2::new(-4.0, -1.0),
            ],
            Stroke::new(2.0, theme::ORANGE),
        );
    }
    paint_icon(
        ui.painter(),
        rect.shrink(size * 0.24),
        icon,
        if active { theme::ORANGE } else { theme::TEXT },
    );
    response
}

pub fn transport_button(ui: &mut Ui, icon: StudioIcon, active: bool, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let fill = if active {
        theme::ORANGE_DIM
    } else if response.hovered() {
        theme::HIGHLIGHT
    } else {
        theme::RAISED
    };
    ui.painter()
        .circle_filled(rect.center() + Vec2::new(0.0, 1.0), 17.0, theme::WELL);
    ui.painter().circle_filled(rect.center(), 16.0, fill);
    ui.painter().circle_stroke(
        rect.center(),
        16.0,
        Stroke::new(1.0, if active { color } else { theme::HIGHLIGHT }),
    );
    paint_icon(ui.painter(), rect.shrink(11.0), icon, color);
    response
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: StudioIcon, color: Color32) {
    let c = rect.center();
    let w = rect.width();
    let h = rect.height();
    let s = Stroke::new((w / 9.0).clamp(1.25, 1.8), color);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], s);
    match icon {
        StudioIcon::Menu => {
            for dy in [-h * 0.32, 0.0, h * 0.32] {
                line(
                    Pos2::new(rect.left(), c.y + dy),
                    Pos2::new(rect.right(), c.y + dy),
                );
            }
        }
        StudioIcon::Undo | StudioIcon::Redo => {
            let flip = if icon == StudioIcon::Undo { -1.0 } else { 1.0 };
            let points = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    let angle = std::f32::consts::PI * (0.15 + t * 0.95);
                    Pos2::new(
                        c.x + flip * angle.cos() * w * 0.43,
                        c.y - angle.sin() * h * 0.38,
                    )
                })
                .collect::<Vec<_>>();
            painter.line(points, s);
            let tip = Pos2::new(c.x - flip * w * 0.42, c.y - h * 0.08);
            line(tip, Pos2::new(tip.x + flip * w * 0.23, tip.y - h * 0.2));
            line(tip, Pos2::new(tip.x + flip * w * 0.23, tip.y + h * 0.2));
        }
        StudioIcon::Folder => {
            let body = Rect::from_min_max(
                Pos2::new(rect.left(), rect.top() + h * 0.25),
                rect.right_bottom(),
            );
            painter.rect_stroke(body, 2.0, s, StrokeKind::Inside);
            line(
                Pos2::new(rect.left() + w * 0.08, rect.top() + h * 0.25),
                Pos2::new(rect.left() + w * 0.36, rect.top()),
            );
            line(
                Pos2::new(rect.left() + w * 0.36, rect.top()),
                Pos2::new(rect.left() + w * 0.58, rect.top() + h * 0.25),
            );
        }
        StudioIcon::Save => {
            painter.rect_stroke(rect, 1.5, s, StrokeKind::Inside);
            painter.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(rect.left() + w * 0.2, rect.top()),
                    Pos2::new(rect.right() - w * 0.2, rect.top() + h * 0.36),
                ),
                0.0,
                s,
                StrokeKind::Inside,
            );
            painter.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(rect.left() + w * 0.2, rect.top() + h * 0.58),
                    Pos2::new(rect.right() - w * 0.2, rect.bottom()),
                ),
                0.0,
                s,
                StrokeKind::Inside,
            );
        }
        StudioIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    rect.left_top(),
                    Pos2::new(rect.right(), c.y),
                    rect.left_bottom(),
                ],
                color,
                Stroke::NONE,
            ));
        }
        StudioIcon::Pause => {
            painter.rect_filled(
                Rect::from_min_max(rect.left_top(), Pos2::new(c.x - w * 0.12, rect.bottom())),
                1.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(c.x + w * 0.12, rect.top()), rect.right_bottom()),
                1.0,
                color,
            );
        }
        StudioIcon::Stop => {
            painter.rect_filled(rect.shrink(w * 0.08), 1.5, color);
        }
        StudioIcon::Record => {
            painter.circle_filled(c, w.min(h) * 0.42, color);
        }
        StudioIcon::Playlist => {
            for i in 0..3 {
                let y = rect.top() + h * (0.15 + i as f32 * 0.35);
                line(Pos2::new(rect.left(), y), Pos2::new(rect.right(), y));
            }
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.left() + w * 0.23, rect.top() + h * 0.07),
                    Vec2::new(w * 0.48, h * 0.18),
                ),
                1.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(rect.left() + w * 0.43, rect.top() + h * 0.43),
                    Vec2::new(w * 0.45, h * 0.18),
                ),
                1.0,
                color,
            );
        }
        StudioIcon::Rack => {
            for i in 0..3 {
                let y = rect.top() + i as f32 * h * 0.38;
                painter.rect_stroke(
                    Rect::from_min_size(Pos2::new(rect.left(), y), Vec2::new(w, h * 0.23)),
                    1.0,
                    s,
                    StrokeKind::Inside,
                );
                painter.circle_filled(
                    Pos2::new(rect.left() + w * 0.16, y + h * 0.115),
                    w * 0.055,
                    color,
                );
            }
        }
        StudioIcon::Piano => {
            painter.rect_stroke(rect, 1.0, s, StrokeKind::Inside);
            for i in 1..4 {
                let x = rect.left() + w * i as f32 / 4.0;
                line(Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
            }
            for i in [1, 2, 3] {
                let x = rect.left() + w * (i as f32 - 0.1) / 4.0;
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w * 0.14, h * 0.55)),
                    0.0,
                    color,
                );
            }
        }
        StudioIcon::Mixer => {
            for (i, knob) in [0.65, 0.32, 0.75].into_iter().enumerate() {
                let x = rect.left() + w * (0.14 + i as f32 * 0.36);
                line(Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
                painter.circle_filled(Pos2::new(x, rect.top() + h * knob), w * 0.12, color);
            }
        }
        StudioIcon::Plugin => {
            painter.rect_stroke(
                rect.shrink2(Vec2::new(w * 0.12, 0.0)),
                2.0,
                s,
                StrokeKind::Inside,
            );
            for y in [rect.top() + h * 0.28, rect.bottom() - h * 0.28] {
                line(
                    Pos2::new(rect.left(), y),
                    Pos2::new(rect.left() + w * 0.12, y),
                );
                line(
                    Pos2::new(rect.right() - w * 0.12, y),
                    Pos2::new(rect.right(), y),
                );
            }
            painter.circle_stroke(c, w * 0.17, s);
        }
        StudioIcon::Search => {
            painter.circle_stroke(Pos2::new(c.x - w * 0.1, c.y - h * 0.1), w * 0.3, s);
            line(
                Pos2::new(c.x + w * 0.12, c.y + h * 0.12),
                rect.right_bottom(),
            );
        }
        StudioIcon::Pointer => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    rect.left_top(),
                    Pos2::new(rect.left() + w * 0.16, rect.bottom()),
                    Pos2::new(rect.left() + w * 0.42, rect.top() + h * 0.68),
                    Pos2::new(rect.left() + w * 0.67, rect.bottom()),
                    Pos2::new(rect.right(), rect.bottom() - h * 0.18),
                    Pos2::new(rect.left() + w * 0.61, rect.top() + h * 0.58),
                    Pos2::new(rect.right(), rect.top() + h * 0.5),
                ],
                color,
                Stroke::NONE,
            ));
        }
        StudioIcon::Pencil => {
            line(
                Pos2::new(rect.left(), rect.bottom()),
                Pos2::new(rect.right() - w * 0.13, rect.top() + h * 0.13),
            );
            line(
                Pos2::new(rect.left() + w * 0.12, rect.bottom()),
                rect.right_top(),
            );
            line(
                Pos2::new(rect.left(), rect.bottom()),
                Pos2::new(rect.left() + w * 0.28, rect.bottom() - h * 0.03),
            );
        }
        StudioIcon::Paint => {
            line(
                Pos2::new(rect.left(), rect.bottom()),
                Pos2::new(rect.right() - w * 0.13, rect.top() + h * 0.13),
            );
            line(
                Pos2::new(rect.left() + w * 0.12, rect.bottom()),
                rect.right_top(),
            );
            for offset in [0.0, 0.24, 0.48] {
                painter.circle_filled(
                    Pos2::new(rect.left() + w * (0.18 + offset), rect.bottom() - h * 0.02),
                    (w * 0.07).max(1.0),
                    color,
                );
            }
        }
        StudioIcon::Eraser => {
            let eraser = Rect::from_center_size(c, Vec2::new(w * 0.78, h * 0.5));
            painter.rect_filled(eraser, 1.0, color);
            line(
                Pos2::new(eraser.center().x, eraser.top()),
                Pos2::new(eraser.center().x, eraser.bottom()),
            );
            line(
                Pos2::new(rect.left(), rect.bottom()),
                Pos2::new(rect.right(), rect.bottom()),
            );
        }
        StudioIcon::Slice => {
            line(
                Pos2::new(rect.left() + w * 0.17, rect.top()),
                Pos2::new(rect.right() - w * 0.17, rect.bottom()),
            );
            painter.circle_stroke(
                Pos2::new(rect.left() + w * 0.16, rect.bottom() - h * 0.12),
                w * 0.15,
                s,
            );
            painter.circle_stroke(
                Pos2::new(rect.right() - w * 0.16, rect.top() + h * 0.12),
                w * 0.15,
                s,
            );
        }
        StudioIcon::Slip => {
            line(Pos2::new(rect.left(), c.y), Pos2::new(rect.right(), c.y));
            line(
                Pos2::new(rect.left(), c.y),
                Pos2::new(rect.left() + w * 0.24, c.y - h * 0.23),
            );
            line(
                Pos2::new(rect.left(), c.y),
                Pos2::new(rect.left() + w * 0.24, c.y + h * 0.23),
            );
            line(
                Pos2::new(rect.right(), c.y),
                Pos2::new(rect.right() - w * 0.24, c.y - h * 0.23),
            );
            line(
                Pos2::new(rect.right(), c.y),
                Pos2::new(rect.right() - w * 0.24, c.y + h * 0.23),
            );
            painter.circle_filled(c, (w * 0.08).max(1.0), color);
        }
        StudioIcon::Fade => {
            let steps = 12;
            let fade_in = (0..=steps).map(|index| {
                let t = index as f32 / steps as f32;
                let gain = (t * std::f32::consts::FRAC_PI_2).sin();
                Pos2::new(rect.left() + t * w, rect.bottom() - gain * h * 0.82)
            });
            let fade_out = (0..=steps).map(|index| {
                let t = index as f32 / steps as f32;
                let gain = (t * std::f32::consts::FRAC_PI_2).sin();
                Pos2::new(rect.left() + t * w, rect.top() + gain * h * 0.82)
            });
            painter.line(fade_in.collect(), s);
            painter.line(fade_out.collect(), s);
        }
        StudioIcon::Mute => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(rect.left(), c.y - h * 0.17),
                    Pos2::new(c.x - w * 0.12, c.y - h * 0.17),
                    Pos2::new(c.x + w * 0.12, rect.top()),
                    Pos2::new(c.x + w * 0.12, rect.bottom()),
                    Pos2::new(c.x - w * 0.12, c.y + h * 0.17),
                    Pos2::new(rect.left(), c.y + h * 0.17),
                ],
                color,
                Stroke::NONE,
            ));
            line(
                Pos2::new(c.x + w * 0.25, c.y - h * 0.2),
                rect.right_bottom(),
            );
            line(
                Pos2::new(rect.right(), c.y - h * 0.2),
                Pos2::new(c.x + w * 0.25, c.y + h * 0.2),
            );
        }
        StudioIcon::Group => {
            let radius = w.min(h) * 0.25;
            painter.circle_stroke(Pos2::new(c.x - w * 0.2, c.y - h * 0.12), radius, s);
            painter.circle_stroke(Pos2::new(c.x + w * 0.2, c.y + h * 0.12), radius, s);
            line(
                Pos2::new(c.x - w * 0.05, c.y - h * 0.02),
                Pos2::new(c.x + w * 0.05, c.y + h * 0.02),
            );
        }
        StudioIcon::Stamp => {
            for (index, offset) in [-0.28_f32, 0.0, 0.28].into_iter().enumerate() {
                let width = w * (0.52 + index as f32 * 0.1);
                painter.rect_filled(
                    Rect::from_center_size(
                        Pos2::new(c.x + w * 0.06, c.y + h * offset),
                        Vec2::new(width, (h * 0.15).max(1.5)),
                    ),
                    1.0,
                    color,
                );
                painter.circle_filled(
                    Pos2::new(c.x - width * 0.5, c.y + h * offset),
                    (w * 0.11).max(1.2),
                    color,
                );
            }
        }
        StudioIcon::Magnet => {
            let left = rect.left() + w * 0.15;
            let right = rect.right() - w * 0.15;
            let bottom = rect.bottom() - h * 0.12;
            line(
                Pos2::new(left, rect.top()),
                Pos2::new(left, bottom - h * 0.2),
            );
            line(Pos2::new(left, bottom - h * 0.2), Pos2::new(c.x, bottom));
            line(Pos2::new(c.x, bottom), Pos2::new(right, bottom - h * 0.2));
            line(
                Pos2::new(right, bottom - h * 0.2),
                Pos2::new(right, rect.top()),
            );
            line(
                Pos2::new(left - w * 0.12, rect.top()),
                Pos2::new(left + w * 0.12, rect.top()),
            );
            line(
                Pos2::new(right - w * 0.12, rect.top()),
                Pos2::new(right + w * 0.12, rect.top()),
            );
        }
        StudioIcon::Browser => {
            painter.rect_stroke(rect, 1.5, s, StrokeKind::Inside);
            line(
                Pos2::new(rect.left() + w * 0.34, rect.top()),
                Pos2::new(rect.left() + w * 0.34, rect.bottom()),
            );
            for dy in [0.27, 0.52, 0.77] {
                painter.circle_filled(
                    Pos2::new(rect.left() + w * 0.17, rect.top() + h * dy),
                    w * 0.045,
                    color,
                );
            }
        }
        StudioIcon::Inspector => {
            painter.rect_stroke(rect, 1.5, s, StrokeKind::Inside);
            line(
                Pos2::new(rect.left(), rect.top() + h * 0.27),
                Pos2::new(rect.right(), rect.top() + h * 0.27),
            );
            for dy in [0.48, 0.72] {
                line(
                    Pos2::new(rect.left() + w * 0.18, rect.top() + h * dy),
                    Pos2::new(rect.right() - w * 0.18, rect.top() + h * dy),
                );
            }
        }
        StudioIcon::Settings => {
            painter.circle_stroke(c, w * 0.22, s);
            painter.circle_stroke(c, w * 0.43, s);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let p1 = Pos2::new(c.x + a.cos() * w * 0.4, c.y + a.sin() * h * 0.4);
                let p2 = Pos2::new(c.x + a.cos() * w * 0.52, c.y + a.sin() * h * 0.52);
                line(p1, p2);
            }
        }
        StudioIcon::More => {
            for dx in [-w * 0.32, 0.0, w * 0.32] {
                painter.circle_filled(Pos2::new(c.x + dx, c.y), w * 0.08, color);
            }
        }
    }
}
