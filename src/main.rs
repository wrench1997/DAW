#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
pub mod audio;
pub mod audio_device;
pub mod automation;
pub mod automation_runtime;
mod clip_fade;
pub mod editor_viewport;
mod export;
mod export_job;
pub mod fixed_quantum;
mod icons;
pub mod master_capture;
pub mod midi;
pub mod midi_device;
pub mod midi_recording;
pub mod midi_runtime;
pub mod mixer_graph;
mod model;
pub mod pdc;
mod piano_roll;
mod playlist;
pub mod plugin_graph;
pub mod plugin_parameter_edit;
pub mod plugin_parameter_editor;
mod plugins;
mod project_media;
pub mod recording;
mod settings_ui;
pub mod tempo_map;
mod theme;
pub mod timeline;
pub mod timeline_automation;
pub mod timeline_executor;
pub mod timeline_plugin_automation;
pub mod timeline_runtime;
pub mod wav;

use app::CitrusApp;
use eframe::egui;
use std::sync::Arc;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Citrus Studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1080.0, 680.0])
            .with_maximized(true)
            .with_icon(Arc::new(citrus_icon()))
            .with_app_id("com.citrus.studio"),
        renderer: eframe::Renderer::Wgpu,
        multisampling: 4,
        depth_buffer: 0,
        stencil_buffer: 0,
        dithering: true,
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "Citrus Studio",
        options,
        Box::new(|cc| Ok(CitrusApp::new_boxed(cc))),
    )
}

fn citrus_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let index = ((y * SIZE + x) * 4) as usize;
            let fruit = ((x as f32 - 30.0).powi(2) + (y as f32 - 35.0).powi(2)).sqrt() < 22.5;
            let leaf = ((x as f32 - 44.0) / 13.0).powi(2) + ((y as f32 - 14.0) / 7.0).powi(2) < 1.0;
            let stem = (29..=34).contains(&x) && (9..=20).contains(&y);
            let color = if leaf {
                Some([86, 210, 164, 255])
            } else if stem {
                Some([74, 126, 84, 255])
            } else if fruit {
                let highlight = x < 24 && y < 30;
                Some(if highlight {
                    [255, 174, 92, 255]
                } else {
                    [255, 126, 55, 255]
                })
            } else {
                None
            };
            if let Some(color) = color {
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
    }
    egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}
