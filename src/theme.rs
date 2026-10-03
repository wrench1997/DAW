use std::{path::Path, sync::Arc};

use egui::{
    Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, Style, TextStyle, Visuals,
};

pub const BG: Color32 = Color32::from_rgb(15, 18, 20);
pub const PANEL: Color32 = Color32::from_rgb(24, 28, 31);
pub const PANEL_ALT: Color32 = Color32::from_rgb(29, 34, 37);
pub const RAISED: Color32 = Color32::from_rgb(38, 44, 47);
pub const GRID: Color32 = Color32::from_rgb(46, 52, 55);
pub const TEXT: Color32 = Color32::from_rgb(218, 224, 224);
pub const MUTED: Color32 = Color32::from_rgb(126, 137, 139);
pub const ORANGE: Color32 = Color32::from_rgb(255, 139, 76);
pub const ORANGE_DIM: Color32 = Color32::from_rgb(125, 69, 42);
pub const GREEN: Color32 = Color32::from_rgb(96, 211, 177);
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

pub fn install(ctx: &egui::Context) {
    install_system_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_zoom_factor(1.04);
    let mut visuals = Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = PANEL_ALT;
    visuals.override_text_color = Some(TEXT);
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(53, 59, 61));
    visuals.widgets.noninteractive.bg_fill = PANEL_ALT;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, GRID);
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.weak_bg_fill = PANEL_ALT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(50, 57, 60));
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(51, 58, 61);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(75, 83, 85));
    visuals.widgets.active.bg_fill = ORANGE_DIM;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, ORANGE);
    visuals.selection.bg_fill = ORANGE_DIM;
    visuals.selection.stroke = Stroke::new(1.0, ORANGE);

    let mut style = Style {
        visuals,
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(7.0, 6.0);
    style.spacing.button_padding = egui::vec2(9.0, 6.0);
    style.spacing.slider_width = 104.0;
    style.spacing.interact_size.y = 28.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(21.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
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
