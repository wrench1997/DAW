use eframe::egui;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_INNER_SIZE: egui::Vec2 = egui::vec2(1440.0, 900.0);
const MIN_INNER_SIZE: egui::Vec2 = egui::vec2(1080.0, 680.0);

pub(super) fn native_options(icon: egui::IconData) -> eframe::NativeOptions {
    let data_dir = std::env::var_os("CITRUS_STUDIO_DATA_DIR");
    native_options_for_data_dir(icon, data_dir.as_deref())
}

fn native_options_for_data_dir(
    icon: egui::IconData,
    data_dir: Option<&OsStr>,
) -> eframe::NativeOptions {
    // Match App's data override validation. eframe 0.35 treats persistence_path
    // as a file, so QA sessions must not share the default window/settings cache.
    let persistence_path = data_dir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join("eframe-app.ron"));

    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Citrus Studio")
            .with_inner_size(DEFAULT_INNER_SIZE)
            .with_min_inner_size(MIN_INNER_SIZE)
            .with_maximized(true)
            .with_icon(Arc::new(icon))
            .with_app_id("com.citrus.studio"),
        // eframe applies this hook after restoring persisted WindowSettings.
        // Fixing only the initial viewport would let the restored tiny size win.
        window_builder: Some(Box::new(sanitize_restored_viewport)),
        persistence_path,
        renderer: eframe::Renderer::Wgpu,
        multisampling: 4,
        depth_buffer: 0,
        stencil_buffer: 0,
        dithering: true,
        persist_window: true,
        ..Default::default()
    }
}

fn sanitize_restored_viewport(mut viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if viewport.inner_size.is_some_and(|size| {
        !size.is_finite() || size.x < MIN_INNER_SIZE.x || size.y < MIN_INNER_SIZE.y
    }) {
        viewport.inner_size = Some(DEFAULT_INNER_SIZE);
        viewport.maximized = Some(true);
    }
    if viewport
        .position
        .is_some_and(|position| !position.is_finite())
    {
        viewport.position = None;
    }
    viewport.min_inner_size = Some(MIN_INNER_SIZE);
    viewport
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restored_viewport(
        mut options: eframe::NativeOptions,
        size: egui::Vec2,
        position: egui::Pos2,
        maximized: bool,
        fullscreen: bool,
    ) -> egui::ViewportBuilder {
        // Exercise the very hook passed to run_native, with the values that
        // WindowSettings restores over the initial defaults before invoking it.
        let restored = options
            .viewport
            .with_inner_size(size)
            .with_position(position)
            .with_maximized(maximized)
            .with_fullscreen(fullscreen);
        options.window_builder.take().expect("startup guard")(restored)
    }

    #[test]
    fn production_hook_repairs_restored_tiny_dimensions() {
        let position = egui::pos2(40.0, 80.0);
        for size in [
            egui::vec2(64.0, 64.0),
            egui::vec2(105.0, 165.0),
            egui::vec2(1079.0, 900.0),
            egui::vec2(1440.0, 679.0),
            egui::vec2(0.0, 900.0),
            egui::vec2(-1.0, 900.0),
        ] {
            let viewport = restored_viewport(
                native_options_for_data_dir(egui::IconData::default(), None),
                size,
                position,
                false,
                false,
            );
            assert_eq!(viewport.inner_size, Some(DEFAULT_INNER_SIZE));
            assert_eq!(viewport.min_inner_size, Some(MIN_INNER_SIZE));
            assert_eq!(viewport.maximized, Some(true));
            assert_eq!(viewport.position, Some(position));
            assert_eq!(viewport.fullscreen, Some(false));
        }
    }

    #[test]
    fn production_hook_repairs_nonfinite_dimensions_and_position() {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for size in [egui::vec2(invalid, 900.0), egui::vec2(1440.0, invalid)] {
                let viewport = restored_viewport(
                    native_options_for_data_dir(egui::IconData::default(), None),
                    size,
                    egui::pos2(invalid, 80.0),
                    false,
                    false,
                );
                assert_eq!(viewport.inner_size, Some(DEFAULT_INNER_SIZE));
                assert_eq!(viewport.min_inner_size, Some(MIN_INNER_SIZE));
                assert_eq!(viewport.maximized, Some(true));
                assert_eq!(viewport.position, None);
            }
            for position in [egui::pos2(invalid, 80.0), egui::pos2(40.0, invalid)] {
                let viewport = restored_viewport(
                    native_options_for_data_dir(egui::IconData::default(), None),
                    egui::vec2(1280.0, 720.0),
                    position,
                    false,
                    true,
                );
                assert_eq!(viewport.inner_size, Some(egui::vec2(1280.0, 720.0)));
                assert_eq!(viewport.maximized, Some(false));
                assert_eq!(viewport.fullscreen, Some(true));
                assert_eq!(viewport.position, None);
            }
        }
    }

    #[test]
    fn production_hook_preserves_legal_restored_geometry_and_window_state() {
        // Negative finite positions can belong to an attached secondary monitor.
        let position = egui::pos2(-1200.0, 140.0);
        for size in [
            MIN_INNER_SIZE,
            egui::vec2(1280.0, 720.0),
            DEFAULT_INNER_SIZE,
        ] {
            for maximized in [false, true] {
                for fullscreen in [false, true] {
                    let viewport = restored_viewport(
                        native_options_for_data_dir(egui::IconData::default(), None),
                        size,
                        position,
                        maximized,
                        fullscreen,
                    );
                    assert_eq!(viewport.inner_size, Some(size));
                    assert_eq!(viewport.min_inner_size, Some(MIN_INNER_SIZE));
                    assert_eq!(viewport.position, Some(position));
                    assert_eq!(viewport.maximized, Some(maximized));
                    assert_eq!(viewport.fullscreen, Some(fullscreen));
                    assert_eq!(viewport.title.as_deref(), Some("Citrus Studio"));
                    assert_eq!(viewport.app_id.as_deref(), Some("com.citrus.studio"));
                    assert!(viewport.icon.is_some());
                }
            }
        }
    }

    #[test]
    fn absolute_data_override_isolates_eframe_persistence_file() {
        let data_dir = std::env::temp_dir().join("citrus-window-startup-qa");
        assert!(data_dir.is_absolute());
        let options =
            native_options_for_data_dir(egui::IconData::default(), Some(data_dir.as_os_str()));
        assert_eq!(
            options.persistence_path,
            Some(data_dir.join("eframe-app.ron"))
        );
        assert!(options.persist_window);
    }

    #[test]
    fn missing_empty_and_relative_overrides_keep_eframe_default_persistence_path() {
        for data_dir in [None, Some(OsStr::new("")), Some(OsStr::new("qa-output"))] {
            let options = native_options_for_data_dir(egui::IconData::default(), data_dir);
            assert_eq!(options.persistence_path, None);
            assert!(options.persist_window);
        }
    }
}
