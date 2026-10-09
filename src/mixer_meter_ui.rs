//! Compact, truthful stereo sample-peak display. This module never mutates a Project.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::{
    audio_meter::{MeterReading, meter_fraction, peak_dbfs},
    theme,
};

// This cadence is requested only by the visible Mixer. It keeps its meter
// receiver current even with stopped transport and no MIDI or transient work.
pub const METER_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

pub fn request_meter_repaint(ctx: &egui::Context) {
    ctx.request_repaint_after(METER_REFRESH_INTERVAL);
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_meter::{
        METER_STALE_AFTER, MeterFrame, MeterIdentity, TrackPeak, meter_channel,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn idle_mixer_requests_bounded_repaint_without_playback_midi_or_transient_work() {
        let ctx = egui::Context::default();
        // Settle egui startup requests. No transport, MIDI, toast, animation or
        // other transient work requests a repaint in this fixture.
        let mut last_delay = Duration::MAX;
        for mixer_visible in [false, false, false, true, true, false, false] {
            let output = ctx.run_ui(
                egui::RawInput {
                    predicted_dt: 0.0,
                    ..Default::default()
                },
                |ui| {
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                    if mixer_visible {
                        request_meter_repaint(ui.ctx());
                    }
                },
            );
            last_delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            if mixer_visible {
                assert_eq!(last_delay, METER_REFRESH_INTERVAL);
                assert!(last_delay < METER_STALE_AFTER / 4);
                assert!(
                    last_delay > Duration::from_millis(16),
                    "idle metering does not request 60Hz"
                );
            }
        }
        assert_eq!(
            last_delay,
            Duration::from_secs(1),
            "hidden Mixer imposes no high-rate repaint on other views"
        );
    }

    #[test]
    fn idle_mixer_cadence_keeps_silence_available_and_clears_stale_audio_promptly() {
        let (mut publisher, mut reader) = meter_channel();
        let identity = MeterIdentity {
            revision: 1,
            epoch: 1,
            graph_fingerprint: 123,
        };
        let start = Instant::now();
        let id = 77;
        reader.poll(start, Some(identity), 0, 48_000);
        let mut device_frame = 0;
        // Sixty visible idle Mixer frames span nearly two seconds, longer than
        // both the old 1-second idle cadence and the 250ms expiry window.
        for tick in 1..=60 {
            publisher.begin_block();
            device_frame += 1584;
            let mut frame = MeterFrame {
                identity: Some(identity),
                end_device_frame: device_frame,
                ..Default::default()
            };
            frame.tracks[1] = TrackPeak::measure(id, &[[0.0; 2]]);
            publisher.publish(frame);
            let reading = reader
                .poll(
                    start + METER_REFRESH_INTERVAL * tick,
                    Some(identity),
                    device_frame,
                    48_000,
                )
                .track(1, id);
            assert!(
                reading.available,
                "a connected silent device must not stay unavailable at idle"
            );
            assert_eq!(reading.peak, [0.0; 2]);
        }
        publisher.begin_block();
        device_frame += 1584;
        let mut frame = MeterFrame {
            identity: Some(identity),
            end_device_frame: device_frame,
            ..Default::default()
        };
        frame.tracks[1] = TrackPeak::measure(id, &[[0.5; 2]]);
        publisher.publish(frame);
        let last_audio = start + METER_REFRESH_INTERVAL * 61;
        assert_eq!(
            reader
                .poll(last_audio, Some(identity), device_frame, 48_000)
                .track(1, id)
                .peak,
            [0.5; 2]
        );
        // Stop callbacks without a separate device fault notification. Visible
        // repaint checks still clear the bars within one cadence of expiry.
        let expiry_ticks = METER_STALE_AFTER
            .as_nanos()
            .div_ceil(METER_REFRESH_INTERVAL.as_nanos()) as u32;
        for tick in 1..expiry_ticks {
            assert!(
                reader
                    .poll(
                        last_audio + METER_REFRESH_INTERVAL * tick,
                        Some(identity),
                        device_frame,
                        48_000
                    )
                    .track(1, id)
                    .available
            );
        }
        let stale = reader
            .poll(
                last_audio + METER_REFRESH_INTERVAL * expiry_ticks,
                Some(identity),
                device_frame,
                48_000,
            )
            .track(1, id);
        assert!(!stale.available);
        assert_eq!(stale.peak, [0.0; 2]);
        assert!(METER_REFRESH_INTERVAL * expiry_ticks < METER_STALE_AFTER + METER_REFRESH_INTERVAL);
    }
}
