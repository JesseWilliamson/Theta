//! Scroll state for the virtualised body. The grid owns the offset (content
//! pixels, non-negative) and implements gpui-kit's `ScrollbarHandle` so the
//! stock scrollbar can drive it; nothing here depends on layout, which is what
//! lets 100k rows exist without 100k elements.

use gpui_kit::component::scroll::ScrollbarHandle;
use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct Inner {
    /// Scroll position in content pixels, `>= 0`.
    x: f32,
    y: f32,
    viewport: Bounds<Pixels>,
    content_w: f32,
    content_h: f32,
}

#[derive(Clone, Default)]
pub struct GridScroll(Rc<RefCell<Inner>>);

impl GridScroll {
    pub fn xy(&self) -> (f32, f32) {
        let i = self.0.borrow();
        (i.x, i.y)
    }

    /// Record the measured body bounds and content size, re-clamping the offset
    /// (a result that shrinks or a window that grows must not leave us scrolled
    /// past the end).
    pub fn update_metrics(&self, viewport: Bounds<Pixels>, content_w: f32, content_h: f32) {
        let mut i = self.0.borrow_mut();
        i.viewport = viewport;
        i.content_w = content_w;
        i.content_h = content_h;
        i.clamp();
    }

    pub fn set(&self, x: f32, y: f32) {
        let mut i = self.0.borrow_mut();
        i.x = x;
        i.y = y;
        i.clamp();
    }

    pub fn scroll_by(&self, dx: f32, dy: f32) -> bool {
        let (x, y) = self.xy();
        self.set(x + dx, y + dy);
        self.xy() != (x, y)
    }
}

impl Inner {
    fn clamp(&mut self) {
        let max_x = (self.content_w - f32::from(self.viewport.size.width)).max(0.0);
        let max_y = (self.content_h - f32::from(self.viewport.size.height)).max(0.0);
        self.x = self.x.clamp(0.0, max_x);
        self.y = self.y.clamp(0.0, max_y);
    }
}

impl ScrollbarHandle for GridScroll {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().viewport
    }

    // GPUI's convention: the offset is the (non-positive) translation of the content.
    fn offset(&self) -> Point<Pixels> {
        let i = self.0.borrow();
        point(px(-i.x), px(-i.y))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set(-f32::from(offset.x), -f32::from(offset.y));
    }

    fn content_size(&self) -> Size<Pixels> {
        let i = self.0.borrow();
        size(px(i.content_w), px(i.content_h))
    }
}
