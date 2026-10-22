use std::{path::Path, sync::Arc};

use egui::{
    Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, Style, TextStyle, Visuals,
};

// Graphite workstation surfaces: dark wells, restrained raised controls and
// warm amber activity. Teal belongs to editable musical content, never chrome.
pub const BG: Color32 = Color32::from_rgb(23, 28, 33);
pub const PANEL: Color32 = Color32::from_rgb(30, 37, 43);
pub const PANEL_ALT: Color32 = Color32::from_rgb(37, 45, 52);
pub const RAISED: Color32 = Color32::from_rgb(45, 54, 62);
pub const GRID: Color32 = Color32::from_rgb(48, 59, 68);
pub const TEXT: Color32 = Color32::from_rgb(229, 233, 238);
pub const MUTED: Color32 = Color32::from_rgb(157, 167, 178);
pub const ORANGE: Color32 = Color32::from_rgb(242, 167, 64);
pub const ORANGE_DIM: Color32 = Color32::from_rgb(75, 56, 33);
pub const CHROME: Color32 = Color32::from_rgb(27, 34, 40);
pub const WELL: Color32 = Color32::from_rgb(18, 24, 29);
pub const HIGHLIGHT: Color32 = Color32::from_rgb(70, 80, 92);
pub const GREEN: Color32 = Color32::from_rgb(76, 190, 166);
pub const RED: Color32 = Color32::from_rgb(244, 92, 106);
pub const BLUE: Color32 = Color32::from_rgb(103, 161, 255);
pub const AMBER: Color32 = Color32::from_rgb(247, 194, 86);

// FL-style System Settings palette. These are intentionally scoped constants:
// the compact blue-gray settings surface should not silently retheme the
// Playlist, Rack, Piano Roll, or Mixer.
pub const SETTINGS_BG: Color32 = Color32::from_rgb(75, 85, 92);
pub const SETTINGS_NAV: Color32 = Color32::from_rgb(58, 68, 75);
pub const SETTINGS_PANEL: Color32 = Color32::from_rgb(65, 75, 82);
pub const SETTINGS_RAISED: Color32 = Color32::from_rgb(94, 104, 110);
pub const SETTINGS_FIELD: Color32 = Color32::from_rgb(53, 63, 69);
pub const SETTINGS_LINE: Color32 = Color32::from_rgb(41, 50, 56);
pub const SETTINGS_TEXT: Color32 = Color32::from_rgb(244, 246, 247);
pub const SETTINGS_MUTED: Color32 = Color32::from_rgb(200, 206, 209);
pub const SETTINGS_ACTIVE: Color32 = Color32::from_rgb(50, 196, 255);
pub const SETTINGS_OK: Color32 = Color32::from_rgb(199, 255, 61);
pub const SETTINGS_WARNING: Color32 = Color32::from_rgb(255, 112, 72);
pub const SETTINGS_ERROR: Color32 = Color32::from_rgb(244, 92, 106);

pub fn color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

pub fn mix(background: Color32, foreground: Color32, amount: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount) as u8;
    Color32::from_rgb(
        channel(background.r(), foreground.r()),
        channel(background.g(), foreground.g()),
        channel(background.b(), foreground.b()),
    )
}

/// Flat workstation controls: tone establishes grouping; a border is reserved for focus.
/// The hit rectangle never changes between rest, hover, press, and keyboard focus.
pub fn button_face(
    rect: egui::Rect,
    fill: Color32,
    _depressed: bool,
    focused: bool,
) -> egui::Shape {
    egui::Shape::Vec(vec![
        egui::Shape::rect_filled(rect, 4, fill),
        egui::Shape::rect_stroke(
            rect.shrink(0.5),
            4,
            Stroke::new(0.7, mix(fill, GRID, 0.45)),
            egui::StrokeKind::Inside,
        ),
        if focused {
            egui::Shape::rect_stroke(
                rect.shrink(0.5),
                3,
                Stroke::new(1.5, ORANGE),
                egui::StrokeKind::Inside,
            )
        } else {
            egui::Shape::Noop
        },
    ])
}

/// Keep egui's native accessibility, keyboard activation, sizing and click semantics.
/// Only the paint underneath the native content is custom.
pub struct Button<'a> {
    native: egui::Button<'a>,
    selected: bool,
}

impl<'a> Button<'a> {
    pub fn new(atoms: impl egui::IntoAtoms<'a>) -> Self {
        Self {
            native: egui::Button::new(atoms),
            selected: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.native = self.native.selected(selected);
        self.selected = selected;
        self
    }

    pub fn truncate(mut self) -> Self {
        self.native = self.native.truncate();
        self
    }
}

impl egui::Widget for Button<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        // Reserve the face underneath the native text without allocating a new widget ID.
        let face_index = ui.painter().add(egui::Shape::Noop);
        let response = ui.add(self.native.fill(Color32::TRANSPARENT).stroke(Stroke::NONE));
        if ui.is_rect_visible(response.rect) {
            let depressed = self.selected || response.is_pointer_button_down_on();
            let fill = if self.selected {
                ORANGE_DIM
            } else if depressed {
                WELL
            } else if response.hovered() {
                HIGHLIGHT
            } else {
                RAISED
            };
            ui.painter().set(
                face_index,
                button_face(response.rect, fill, depressed, response.has_focus()),
            );
        }
        response
    }
}

pub trait UiButtons {
    fn studio_button<'a>(&mut self, atoms: impl egui::IntoAtoms<'a>) -> egui::Response;
}

impl UiButtons for egui::Ui {
    fn studio_button<'a>(&mut self, atoms: impl egui::IntoAtoms<'a>) -> egui::Response {
        self.add(Button::new(atoms))
    }
}

/// Keep small white clip titles readable for arbitrary user-assigned colors.
/// This affects paint only; the project retains its exact chosen track color.
pub fn clip_header(color: Color32, muted: bool) -> Color32 {
    let mut amount = if muted { 0.2 } else { 0.48 };
    loop {
        let fill = mix(WELL, color, amount);
        if relative_luminance(fill) <= 0.18 || amount <= 0.2 {
            return fill;
        }
        amount -= 0.02;
    }
}

fn relative_luminance(color: Color32) -> f32 {
    let linear = |channel: u8| {
        let value = f32::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

pub fn install(ctx: &egui::Context) {
    install_system_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_zoom_factor(1.0);
    let mut visuals = Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.window_highlight_topmost = true;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = PANEL_ALT;
    visuals.override_text_color = Some(TEXT);
    visuals.window_stroke = Stroke::new(1.0, GRID);
    visuals.window_corner_radius = egui::CornerRadius::same(5);
    visuals.widgets.noninteractive.bg_fill = PANEL_ALT;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, GRID);
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.weak_bg_fill = PANEL_ALT;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.bg_fill = HIGHLIGHT;
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.bg_fill = WELL;
    visuals.widgets.active.weak_bg_fill = WELL;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ORANGE);
    visuals.selection.bg_fill = ORANGE_DIM;
    visuals.selection.stroke = Stroke::new(1.0, ORANGE);

    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(5.0, 4.0);
    style.spacing.button_padding = egui::vec2(7.0, 3.0);
    style.spacing.slider_width = 104.0;
    style.spacing.interact_size.y = 25.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(18.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
        (
            TextStyle::Button,
            FontId::new(12.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(10.5, FontFamily::Proportional),
        ),
    ]
    .into();
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn install_system_fonts(ctx: &egui::Context) {
    let candidates = [
        ("Segoe UI", "C:/Windows/Fonts/segoeui.ttf"),
        ("Microsoft YaHei", "C:/Windows/Fonts/msyh.ttc"),
    ];
    let mut fonts = FontDefinitions::default();
    let mut installed = Vec::new();
    for (name, path) in candidates {
        let Ok(bytes) = std::fs::read(Path::new(path)) else {
            continue;
        };
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
        installed.push(name.to_owned());
    }
    if installed.is_empty() {
        return;
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        for name in installed.into_iter().rev() {
            family.insert(0, name);
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphite_text_and_focus_keep_a_readable_visual_hierarchy() {
        for background in [BG, PANEL, PANEL_ALT, WELL] {
            let contrast =
                (relative_luminance(MUTED) + 0.05) / (relative_luminance(background) + 0.05);
            assert!(contrast >= 4.5, "secondary text contrast {contrast}");
        }
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(28.0, 28.0));
        let egui::Shape::Vec(shapes) = button_face(rect, PANEL, false, true) else {
            panic!("button paint must retain its independent focus layer");
        };
        assert!(shapes.iter().any(|shape| matches!(shape,
            egui::Shape::Rect(face) if face.stroke.color == ORANGE && face.stroke.width >= 1.0
        )));
    }

    #[test]
    fn clip_title_contrast_survives_bright_custom_colors() {
        for r in [0, 64, 128, 192, 255] {
            for g in [0, 64, 128, 192, 255] {
                for b in [0, 64, 128, 192, 255] {
                    for muted in [false, true] {
                        let fill = clip_header(Color32::from_rgb(r, g, b), muted);
                        assert!(1.05 / (relative_luminance(fill) + 0.05) >= 4.5);
                    }
                }
            }
        }
    }
}
