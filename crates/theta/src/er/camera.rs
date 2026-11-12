//! The diagram's viewport: which part of the world is visible and how big it is drawn. Pure arithmetic, so
//! the zoom-around-the-pointer and fit rules have tests.

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 3.0;

/// World coordinate of the viewport's top-left corner and the scale (screen pixels per world unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub ox: f32,
    pub oy: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { ox: 0.0, oy: 0.0, zoom: 1.0 }
    }
}

impl Camera {
    pub fn to_screen(&self, wx: f32, wy: f32) -> (f32, f32) {
        ((wx - self.ox) * self.zoom, (wy - self.oy) * self.zoom)
    }

    pub fn to_world(&self, sx: f32, sy: f32) -> (f32, f32) {
        (sx / self.zoom + self.ox, sy / self.zoom + self.oy)
    }

    /// Show all of `content` (width, height), centred, never magnified past 100% (a three-table diagram
    /// should not fill the window with giant boxes).
    pub fn fit(content: (f32, f32), view: (f32, f32)) -> Camera {
        let (cw, ch) = (content.0.max(1.0), content.1.max(1.0));
        let zoom = (view.0 / cw).min(view.1 / ch).clamp(MIN_ZOOM, 1.0);
        Camera { ox: (cw - view.0 / zoom) / 2.0, oy: (ch - view.1 / zoom) / 2.0, zoom }
    }

    /// Drag by a screen-space amount: the world follows the pointer.
    pub fn pan_screen(&mut self, dx: f32, dy: f32) {
        self.ox -= dx / self.zoom;
        self.oy -= dy / self.zoom;
    }

    /// Scale by `factor`, keeping the world point under screen point `(sx, sy)` where it is.
    pub fn zoom_at(&mut self, factor: f32, sx: f32, sy: f32) {
        let (wx, wy) = self.to_world(sx, sy);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.ox = wx - sx / self.zoom;
        self.oy = wy - sy / self.zoom;
    }

    pub fn set_zoom_centered(&mut self, zoom: f32, view: (f32, f32)) {
        let factor = zoom / self.zoom;
        self.zoom_at(factor, view.0 / 2.0, view.1 / 2.0);
    }

    /// Centre the world rectangle `(x, y, w, h)` in the viewport at the current zoom.
    pub fn centre_on(&mut self, rect: (f32, f32, f32, f32), view: (f32, f32)) {
        self.ox = rect.0 + rect.2 / 2.0 - view.0 / self.zoom / 2.0;
        self.oy = rect.1 + rect.3 / 2.0 - view.1 / self.zoom / 2.0;
    }

    /// Whether the rectangle is completely inside the viewport.
    pub fn contains(&self, rect: (f32, f32, f32, f32), view: (f32, f32)) -> bool {
        let (x0, y0) = self.to_screen(rect.0, rect.1);
        let (x1, y1) = self.to_screen(rect.0 + rect.2, rect.1 + rect.3);
        x0 >= 0.0 && y0 >= 0.0 && x1 <= view.0 && y1 <= view.1
    }

    /// Bring a rectangle into view: nothing happens if it already is, otherwise it is centred.
    pub fn reveal(&mut self, rect: (f32, f32, f32, f32), view: (f32, f32)) {
        if !self.contains(rect, view) {
            self.centre_on(rect, view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let mut c = Camera { ox: 100.0, oy: 50.0, zoom: 0.5 };
        let before = c.to_world(300.0, 200.0);
        c.zoom_at(2.0, 300.0, 200.0);
        let after = c.to_world(300.0, 200.0);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        assert!((c.zoom - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut c = Camera::default();
        for _ in 0..50 {
            c.zoom_at(1.5, 0.0, 0.0);
        }
        assert_eq!(c.zoom, MAX_ZOOM);
        for _ in 0..100 {
            c.zoom_at(0.5, 0.0, 0.0);
        }
        assert_eq!(c.zoom, MIN_ZOOM);
    }

    #[test]
    fn fit_shows_everything_centred() {
        let c = Camera::fit((2000.0, 1000.0), (1000.0, 800.0));
        assert!((c.zoom - 0.5).abs() < 1e-6);
        // The content's centre is the viewport's centre.
        let (sx, sy) = c.to_screen(1000.0, 500.0);
        assert!((sx - 500.0).abs() < 1e-3 && (sy - 400.0).abs() < 1e-3);
        assert!(c.contains((0.0, 0.0, 2000.0, 1000.0), (1000.0, 800.0)));
    }

    #[test]
    fn fit_does_not_magnify_a_small_diagram() {
        assert_eq!(Camera::fit((300.0, 200.0), (1600.0, 900.0)).zoom, 1.0);
    }

    #[test]
    fn dragging_moves_the_world_with_the_pointer() {
        let mut c = Camera { ox: 10.0, oy: 10.0, zoom: 2.0 };
        let p = c.to_screen(50.0, 50.0);
        c.pan_screen(30.0, -20.0);
        let q = c.to_screen(50.0, 50.0);
        assert!((q.0 - p.0 - 30.0).abs() < 1e-3 && (q.1 - p.1 + 20.0).abs() < 1e-3);
    }

    #[test]
    fn reveal_only_moves_when_needed() {
        let mut c = Camera::default();
        let before = c;
        c.reveal((10.0, 10.0, 100.0, 50.0), (800.0, 600.0));
        assert_eq!(c, before);
        c.reveal((5000.0, 10.0, 100.0, 50.0), (800.0, 600.0));
        let (sx, _) = c.to_screen(5050.0, 35.0);
        assert!((sx - 400.0).abs() < 1e-3);
    }
}
