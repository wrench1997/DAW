//! Shared, deterministic two-dimensional editor viewport math.
//!
//! Playlist and Piano Roll use different content units, but scrolling, anchored zoom,
//! reveal-selection and resize clamping must obey the same rules. This module deliberately has
//! no egui dependency so interaction code can be tested without constructing a window.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportError {
    NonFinite,
    EmptyContentRange,
    InvalidViewportExtent,
    InvalidScaleRange,
    InvalidZoomFactor,
}

impl fmt::Display for ViewportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid editor viewport: {self:?}")
    }
}

impl std::error::Error for ViewportError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisViewport {
    content_min: f64,
    content_max: f64,
    origin: f64,
    viewport_pixels: f64,
    pixels_per_unit: f64,
    minimum_pixels_per_unit: f64,
    maximum_pixels_per_unit: f64,
}

impl AxisViewport {
    pub fn new(
        content_min: f64,
        content_max: f64,
        viewport_pixels: f64,
        pixels_per_unit: f64,
        minimum_pixels_per_unit: f64,
        maximum_pixels_per_unit: f64,
    ) -> Result<Self, ViewportError> {
        validate_finite([
            content_min,
            content_max,
            viewport_pixels,
            pixels_per_unit,
            minimum_pixels_per_unit,
            maximum_pixels_per_unit,
        ])?;
        if content_max <= content_min {
            return Err(ViewportError::EmptyContentRange);
        }
        if viewport_pixels <= 0.0 {
            return Err(ViewportError::InvalidViewportExtent);
        }
        if minimum_pixels_per_unit <= 0.0
            || maximum_pixels_per_unit < minimum_pixels_per_unit
            || pixels_per_unit <= 0.0
        {
            return Err(ViewportError::InvalidScaleRange);
        }
        let mut viewport = Self {
            content_min,
            content_max,
            origin: content_min,
            viewport_pixels,
            pixels_per_unit: pixels_per_unit
                .clamp(minimum_pixels_per_unit, maximum_pixels_per_unit),
            minimum_pixels_per_unit,
            maximum_pixels_per_unit,
        };
        viewport.clamp_origin();
        Ok(viewport)
    }

    #[must_use]
    pub const fn content_range(self) -> (f64, f64) {
        (self.content_min, self.content_max)
    }

    #[must_use]
    pub const fn origin(self) -> f64 {
        self.origin
    }

    #[must_use]
    pub const fn pixels_per_unit(self) -> f64 {
        self.pixels_per_unit
    }

    #[must_use]
    pub const fn viewport_pixels(self) -> f64 {
        self.viewport_pixels
    }

    #[must_use]
    pub fn content_at_pixel(self, pixel: f64) -> f64 {
        self.origin + pixel / self.pixels_per_unit
    }

    #[must_use]
    pub fn pixel_for_content(self, content: f64) -> f64 {
        (content - self.origin) * self.pixels_per_unit
    }

    #[must_use]
    pub fn visible_range(self) -> (f64, f64) {
        (
            self.origin.max(self.content_min),
            (self.origin + self.visible_units()).min(self.content_max),
        )
    }

    #[must_use]
    pub fn normalized_scroll(self) -> f64 {
        let travel = self.maximum_origin() - self.content_min;
        if travel <= f64::EPSILON {
            0.0
        } else {
            ((self.origin - self.content_min) / travel).clamp(0.0, 1.0)
        }
    }

    pub fn set_normalized_scroll(&mut self, normalized: f64) -> Result<(), ViewportError> {
        if !normalized.is_finite() {
            return Err(ViewportError::NonFinite);
        }
        let travel = self.maximum_origin() - self.content_min;
        self.origin = self.content_min + travel * normalized.clamp(0.0, 1.0);
        self.clamp_origin();
        Ok(())
    }

    /// Scroll the visible window in screen-pixel units. Positive values move toward larger
    /// content coordinates; this matches scrollbar/wheel deltas rather than drag-to-pan deltas.
    pub fn scroll_by_pixels(&mut self, delta_pixels: f64) -> Result<(), ViewportError> {
        if !delta_pixels.is_finite() {
            return Err(ViewportError::NonFinite);
        }
        self.origin += delta_pixels / self.pixels_per_unit;
        self.clamp_origin();
        Ok(())
    }

    /// Zoom around a screen-space anchor while preserving the content coordinate under it.
    pub fn zoom_at_pixel(&mut self, anchor_pixel: f64, factor: f64) -> Result<(), ViewportError> {
        if !anchor_pixel.is_finite() || !factor.is_finite() {
            return Err(ViewportError::NonFinite);
        }
        if factor <= 0.0 {
            return Err(ViewportError::InvalidZoomFactor);
        }
        let anchor_content = self.content_at_pixel(anchor_pixel);
        self.pixels_per_unit = (self.pixels_per_unit * factor)
            .clamp(self.minimum_pixels_per_unit, self.maximum_pixels_per_unit);
        self.origin = anchor_content - anchor_pixel / self.pixels_per_unit;
        self.clamp_origin();
        Ok(())
    }

    pub fn set_viewport_pixels(&mut self, viewport_pixels: f64) -> Result<(), ViewportError> {
        if !viewport_pixels.is_finite() {
            return Err(ViewportError::NonFinite);
        }
        if viewport_pixels <= 0.0 {
            return Err(ViewportError::InvalidViewportExtent);
        }
        self.viewport_pixels = viewport_pixels;
        self.clamp_origin();
        Ok(())
    }

    pub fn set_content_range(
        &mut self,
        content_min: f64,
        content_max: f64,
    ) -> Result<(), ViewportError> {
        validate_finite([content_min, content_max])?;
        if content_max <= content_min {
            return Err(ViewportError::EmptyContentRange);
        }
        self.content_min = content_min;
        self.content_max = content_max;
        self.clamp_origin();
        Ok(())
    }

    /// Scroll just enough to expose `[start, end]` plus a pixel margin. Oversized ranges expose
    /// their leading edge deterministically.
    pub fn reveal(
        &mut self,
        start: f64,
        end: f64,
        margin_pixels: f64,
    ) -> Result<(), ViewportError> {
        validate_finite([start, end, margin_pixels])?;
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let margin = margin_pixels.max(0.0) / self.pixels_per_unit;
        let wanted_start = (start - margin).max(self.content_min);
        let wanted_end = (end + margin).min(self.content_max);
        let visible_units = self.visible_units();
        if wanted_end - wanted_start >= visible_units || wanted_start < self.origin {
            self.origin = wanted_start;
        } else if wanted_end > self.origin + visible_units {
            self.origin = wanted_end - visible_units;
        }
        self.clamp_origin();
        Ok(())
    }

    fn visible_units(self) -> f64 {
        self.viewport_pixels / self.pixels_per_unit
    }

    fn maximum_origin(self) -> f64 {
        (self.content_max - self.visible_units()).max(self.content_min)
    }

    fn clamp_origin(&mut self) {
        self.origin = self.origin.clamp(self.content_min, self.maximum_origin());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport2D {
    pub x: AxisViewport,
    pub y: AxisViewport,
}

impl Viewport2D {
    #[must_use]
    pub const fn new(x: AxisViewport, y: AxisViewport) -> Self {
        Self { x, y }
    }

    pub fn scroll_by_pixels(&mut self, x: f64, y: f64) -> Result<(), ViewportError> {
        // Validate both before mutating either axis, preserving transactional behavior.
        validate_finite([x, y])?;
        let mut next = *self;
        next.x.scroll_by_pixels(x)?;
        next.y.scroll_by_pixels(y)?;
        *self = next;
        Ok(())
    }

    pub fn zoom_at_pixel(
        &mut self,
        anchor_x: f64,
        anchor_y: f64,
        factor_x: f64,
        factor_y: f64,
    ) -> Result<(), ViewportError> {
        let mut next = *self;
        next.x.zoom_at_pixel(anchor_x, factor_x)?;
        next.y.zoom_at_pixel(anchor_y, factor_y)?;
        *self = next;
        Ok(())
    }
}

fn validate_finite<const N: usize>(values: [f64; N]) -> Result<(), ViewportError> {
    if values.into_iter().all(f64::is_finite) {
        Ok(())
    } else {
        Err(ViewportError::NonFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playlist() -> Viewport2D {
        Viewport2D::new(
            AxisViewport::new(0.0, 4_096.0, 1_200.0, 8.0, 1.0, 256.0).unwrap(),
            AxisViewport::new(0.0, 32.0, 640.0, 32.0, 12.0, 96.0).unwrap(),
        )
    }

    #[test]
    fn playlist_all_tracks_and_full_song_are_reachable() {
        let mut viewport = playlist();
        viewport.x.set_normalized_scroll(1.0).unwrap();
        viewport.y.set_normalized_scroll(1.0).unwrap();
        assert_eq!(viewport.x.visible_range().1, 4_096.0);
        assert_eq!(viewport.y.visible_range(), (12.0, 32.0));

        viewport.y.reveal(31.0, 32.0, 0.0).unwrap();
        assert!(viewport.y.visible_range().1 >= 32.0);
        viewport.y.reveal(0.0, 1.0, 0.0).unwrap();
        assert_eq!(viewport.y.visible_range().0, 0.0);
    }

    #[test]
    fn piano_midi_zero_and_127_are_reachable() {
        let mut pitch = AxisViewport::new(0.0, 128.0, 576.0, 24.0, 8.0, 96.0).unwrap();
        assert_eq!(pitch.visible_range(), (0.0, 24.0));
        pitch.reveal(127.0, 128.0, 0.0).unwrap();
        assert_eq!(pitch.visible_range(), (104.0, 128.0));
        pitch.reveal(0.0, 1.0, 0.0).unwrap();
        assert_eq!(pitch.visible_range().0, 0.0);
    }

    #[test]
    fn anchored_zoom_is_stable_at_common_dpi_scales() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let mut axis =
                AxisViewport::new(0.0, 4_096.0, 900.0 * dpi, 10.0 * dpi, 1.0, 512.0).unwrap();
            axis.scroll_by_pixels(2_500.0 * dpi).unwrap();
            let anchor = 417.25 * dpi;
            let before = axis.content_at_pixel(anchor);
            axis.zoom_at_pixel(anchor, 1.7).unwrap();
            let after = axis.content_at_pixel(anchor);
            let pixel_error = (after - before).abs() * axis.pixels_per_unit();
            assert!(pixel_error < 0.5, "DPI {dpi}: {pixel_error} px");
        }
    }

    #[test]
    fn resize_and_content_shrink_clamp_without_overscroll() {
        let mut axis = AxisViewport::new(0.0, 100.0, 20.0, 1.0, 0.5, 8.0).unwrap();
        axis.set_normalized_scroll(1.0).unwrap();
        assert_eq!(axis.origin(), 80.0);
        axis.set_viewport_pixels(60.0).unwrap();
        assert_eq!(axis.origin(), 40.0);
        axis.set_content_range(0.0, 30.0).unwrap();
        assert_eq!(axis.origin(), 0.0);
        assert_eq!(axis.visible_range(), (0.0, 30.0));
    }

    #[test]
    fn two_axis_updates_are_transactional_on_invalid_input() {
        let mut viewport = playlist();
        let original = viewport;
        assert_eq!(
            viewport.scroll_by_pixels(16.0, f64::NAN),
            Err(ViewportError::NonFinite)
        );
        assert_eq!(viewport, original);
        assert_eq!(
            viewport.zoom_at_pixel(10.0, 10.0, 2.0, 0.0),
            Err(ViewportError::InvalidZoomFactor)
        );
        assert_eq!(viewport, original);
    }

    #[test]
    fn invalid_construction_and_reversed_reveal_are_deterministic() {
        assert_eq!(
            AxisViewport::new(1.0, 1.0, 100.0, 1.0, 1.0, 2.0),
            Err(ViewportError::EmptyContentRange)
        );
        let mut axis = AxisViewport::new(0.0, 100.0, 10.0, 1.0, 1.0, 4.0).unwrap();
        axis.reveal(80.0, 70.0, 1.0).unwrap();
        // The requested interval plus margins is wider than the viewport, so
        // the documented deterministic behavior exposes its leading edge.
        assert_eq!(axis.visible_range(), (69.0, 79.0));
    }
}
