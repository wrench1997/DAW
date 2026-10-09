//! Compact, truthful stereo sample-peak display. This module never mutates a Project.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::{
    audio_meter::{MeterReading, meter_fraction, peak_dbfs},
    theme,
};

/// Returns true when the user requests a peak-hold/clip reset.
pub fn draw_meter(ui: &mut egui::Ui, reading: MeterReading, master: bool, height: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(58.0, height), Sense::click());
    let painter = ui.painter();
    let usable = reading.available && !reading.invalid;
    let heading = if reading.invalid {
        "FAULT"
    } else if reading.clipped {
        "CLIP"
    } else {
        "L      R"
    };
    painter.text(
        Pos2::new(rect.center().x, rect.top()),
        Align2::CENTER_TOP,
        heading,
        FontId::monospace(8.0),
        if reading.clipped || reading.invalid {
            theme::RED
        } else {
            theme::MUTED
        },
    );
    let bars_rect = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + 12.0),
        Pos2::new(rect.right(), rect.bottom() - 12.0),
    );
    let bars = 18;
    for channel in 0..2 {
        let x = bars_rect.left() + channel as f32 * 31.0;
        for bar in 0..bars {
            let t = (bar + 1) as f32 / bars as f32;
            let db = -60.0 + 60.0 * t;
            let color = if db >= -3.0 {
                theme::RED
            } else if db >= -12.0 {
                theme::ORANGE
            } else {
                theme::GREEN
            };
            // A zero peak illuminates no segment, including the bottom segment.
            let active = usable && t <= meter_fraction(reading.peak[channel]);
            painter.rect_filled(
                Rect::from_min_size(
                    Pos2::new(x, bars_rect.bottom() - t * bars_rect.height()),
                    Vec2::new(27.0, 2.0),
                ),
                0.0,
                color.gamma_multiply(if active { 0.9 } else { 0.13 }),
            );
        }
        if usable && meter_fraction(reading.held_peak[channel]) > 0.0 {
            let y = bars_rect.bottom()
                - meter_fraction(reading.held_peak[channel]) * bars_rect.height();
            painter.line_segment(
                [Pos2::new(x, y), Pos2::new(x + 27.0, y)],
                Stroke::new(1.0, theme::TEXT),
            );
        }
    }
    let label = if !usable {
        "—".into()
    } else {
        peak_dbfs(reading.peak[0].max(reading.peak[1]))
            .map_or_else(|| "−inf".into(), |db| format!("{db:+.1}"))
    };
    painter.text(
        Pos2::new(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        label,
        FontId::monospace(8.0),
        theme::MUTED,
    );
    let tap = if master {
        "MASTER stereo bus after effects/fader/pan/mute, before output tanh protection and device mono fold-down/clamp. CLIP means this bus reached 0 dBFS; it does not claim the protected device output clipped."
    } else {
        "Stereo bus after effects/fader/pan/mute, before post-fader sends. Pre-fader sends can remain audible when this bus meter reads zero."
    };
    let status = if !reading.available {
        "\nNo current measurement for this graph/device."
    } else if reading.invalid {
        "\nNon-finite samples observed; level unavailable."
    } else {
        ""
    };
    let response = response.on_hover_text(format!(
        "{tap}\nSample peak, dBFS. Scale −60 to 0 dBFS. White marker holds for 1 second. Click to reset hold/CLIP. No RMS, loudness or true-peak measurement.{status}"));
    response.clicked()
}
