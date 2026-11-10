//! Scroll, visibility and popup-placement arithmetic. Pure `f32`s, no GPUI: every rule the
//! element applies is unit-tested here.
//!
//! Coordinates: `scroll_y`/`scroll_x` are how far the content has moved up/left (>= 0).
//! Text x positions are relative to the start of the text column (gutter excluded).

use quarry_editor::ScrollRequest;
use std::ops::Range;

/// Lines of context kept around the cursor by `RevealCursor`.
pub const REVEAL_LINES: f32 = 3.0;
/// Columns of context kept to the left/right of the cursor by `RevealCursor`.
pub const REVEAL_COLUMNS: f32 = 5.0;
/// Extra lines shaped above/below the viewport so fast wheel scrolls never show unpainted rows.
pub const OVERSCAN_LINES: usize = 2;

/// Largest vertical scroll offset: the last line may rise to the middle of the viewport
/// (half a viewport of overscroll, as in Zed).
pub fn max_scroll_y(line_count: usize, line_height: f32, viewport_h: f32) -> f32 {
    (line_count as f32 * line_height - viewport_h * 0.5).max(0.0)
}

/// Largest horizontal scroll offset. `trailing` leaves room to see the caret after the
/// longest line.
pub fn max_scroll_x(content_w: f32, viewport_w: f32, trailing: f32) -> f32 {
    (content_w + trailing - viewport_w).max(0.0)
}

/// Lines intersecting the viewport (no overscan): `first..last_exclusive`.
pub fn visible_lines(scroll_y: f32, line_height: f32, viewport_h: f32, line_count: usize) -> Range<usize> {
    if line_count == 0 || line_height <= 0.0 {
        return 0..0;
    }
    let first = (scroll_y / line_height).floor().max(0.0) as usize;
    let last = ((scroll_y + viewport_h) / line_height).ceil().max(0.0) as usize;
    first.min(line_count)..last.min(line_count).max(first.min(line_count))
}

/// The visible range widened by `OVERSCAN_LINES` on both sides (what gets shaped).
pub fn shaped_lines(visible: &Range<usize>, line_count: usize) -> Range<usize> {
    visible.start.saturating_sub(OVERSCAN_LINES)..(visible.end + OVERSCAN_LINES).min(line_count)
}

/// Number of lines that fit fully (what the engine calls the viewport height: it pages by it).
pub fn full_lines_in_viewport(line_height: f32, viewport_h: f32) -> usize {
    ((viewport_h / line_height).floor() as usize).max(1)
}

/// Minimal scroll that brings the cursor cell into view with margins. `cursor_x`/`cursor_w`
/// are in text-column coordinates.
#[allow(clippy::too_many_arguments)]
pub fn reveal(
    (scroll_x, scroll_y): (f32, f32),
    cursor_line: usize,
    cursor_x: f32,
    cursor_w: f32,
    line_height: f32,
    cell_w: f32,
    viewport: (f32, f32),
) -> (f32, f32) {
    let (vw, vh) = viewport;
    // Never ask for more margin than fits: in a tiny viewport the margin shrinks so the
    // cursor can still satisfy both edges.
    let lines_visible = (vh / line_height).max(1.0);
    let margin_y = REVEAL_LINES.min(((lines_visible - 1.0) / 2.0).floor().max(0.0)) * line_height;
    let top = cursor_line as f32 * line_height;
    let bottom = top + line_height;
    let mut y = scroll_y;
    if top - margin_y < y {
        y = top - margin_y;
    } else if bottom + margin_y > y + vh {
        y = bottom + margin_y - vh;
    }
    let cols_visible = (vw / cell_w).max(1.0);
    let margin_x = REVEAL_COLUMNS.min(((cols_visible - 1.0) / 2.0).floor().max(0.0)) * cell_w;
    let mut x = scroll_x;
    if cursor_x - margin_x < x {
        x = cursor_x - margin_x;
    } else if cursor_x + cursor_w + margin_x > x + vw {
        x = cursor_x + cursor_w + margin_x - vw;
    }
    (x.max(0.0), y.max(0.0))
}

/// New `scroll_y` for an engine scroll request (`zz`, `zt`, `zb`, ctrl-e/ctrl-y ...).
pub fn scroll_for_request(
    req: ScrollRequest,
    cursor_line: usize,
    scroll_y: f32,
    line_height: f32,
    viewport_h: f32,
) -> f32 {
    let top = cursor_line as f32 * line_height;
    match req {
        ScrollRequest::CursorCenter => top + line_height * 0.5 - viewport_h * 0.5,
        ScrollRequest::CursorTop => top,
        ScrollRequest::CursorBottom => top + line_height - viewport_h,
        ScrollRequest::Lines(n) => scroll_y + n as f32 * line_height,
    }
}

/// Line index under a y coordinate inside the text area (clamped to the buffer).
pub fn line_at_y(y_in_view: f32, scroll_y: f32, line_height: f32, line_count: usize) -> usize {
    (((y_in_view + scroll_y) / line_height).floor().max(0.0) as usize).min(line_count.saturating_sub(1))
}

/// Gutter width: lane for statement marks, right-aligned numbers wide enough for the largest
/// number, and breathing room before the text.
pub fn gutter_width(line_count: usize, cell_w: f32, lane: f32, left_pad: f32, right_pad: f32) -> f32 {
    let digits = line_count.max(1).to_string().len().max(2) as f32;
    lane + left_pad + digits * cell_w + right_pad
}

/// Vertical scrollbar thumb: `(top, height)` inside a track of `track_h`.
pub fn thumb_metrics(
    scroll: f32,
    max_scroll: f32,
    viewport: f32,
    track_h: f32,
    min_thumb: f32,
) -> (f32, f32) {
    let content = viewport + max_scroll;
    if content <= 0.0 || max_scroll <= 0.0 {
        return (0.0, track_h);
    }
    let h = (viewport / content * track_h).clamp(min_thumb.min(track_h), track_h);
    let travel = track_h - h;
    (travel * (scroll / max_scroll).clamp(0.0, 1.0), h)
}

/// Inverse of `thumb_metrics`: the scroll offset for a thumb top position.
pub fn scroll_for_thumb(thumb_top: f32, thumb_h: f32, track_h: f32, max_scroll: f32) -> f32 {
    let travel = (track_h - thumb_h).max(0.0001);
    (thumb_top / travel).clamp(0.0, 1.0) * max_scroll
}

/// Edge auto-scroll while dragging a selection: pixels to scroll per tick for a pointer
/// `dist` pixels beyond the edge. Grows with distance, capped.
pub fn autoscroll_step(dist: f32, line_height: f32) -> f32 {
    if dist <= 0.0 {
        return 0.0;
    }
    (dist * 0.35 + 2.0).min(line_height * 3.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub above: bool,
}

/// Where to put a popup of `(w, h)` under the cell at `(anchor_x, anchor_y)` of height
/// `anchor_h`. Prefers below, flips above when it does not fit and the other side has more
/// room, and always clamps into the window.
pub fn place_popup(
    anchor: (f32, f32, f32),
    size: (f32, f32),
    window: (f32, f32),
    gap: f32,
    margin: f32,
) -> Placement {
    let (ax, ay, ah) = anchor;
    let (w, h) = size;
    let (ww, wh) = window;
    let x = ax.min(ww - w - margin).max(margin);
    let below_room = wh - (ay + ah + gap) - margin;
    let above_room = ay - gap - margin;
    let above = h > below_room && above_room > below_room;
    let y = if above { (ay - gap - h).max(margin) } else { (ay + ah + gap).min(wh - h - margin).max(margin) };
    Placement { x, y, above }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LH: f32 = 21.0;

    #[test]
    fn overscroll_is_half_a_viewport() {
        // 100 lines of 21px = 2100px; viewport 400 -> may scroll until the last line is mid-view.
        assert_eq!(max_scroll_y(100, LH, 400.0), 2100.0 - 200.0);
        // Short buffers never scroll.
        assert_eq!(max_scroll_y(3, LH, 400.0), 0.0);
    }

    #[test]
    fn horizontal_range_includes_trailing_room() {
        assert_eq!(max_scroll_x(1000.0, 400.0, 20.0), 620.0);
        assert_eq!(max_scroll_x(100.0, 400.0, 20.0), 0.0);
    }

    #[test]
    fn visible_range_partial_lines() {
        // Scrolled 10px into line 1: lines 1..(ceil((10+100)/21)=6)
        assert_eq!(visible_lines(10.0 + LH, LH, 100.0, 1000), 1..7);
        assert_eq!(visible_lines(0.0, LH, 100.0, 3), 0..3);
        assert_eq!(visible_lines(0.0, LH, 100.0, 0), 0..0);
        // Past the end clamps to an empty range at the end.
        assert_eq!(visible_lines(100_000.0, LH, 100.0, 50), 50..50);
    }

    #[test]
    fn shaped_range_adds_overscan() {
        assert_eq!(shaped_lines(&(10..20), 100), 8..22);
        assert_eq!(shaped_lines(&(0..5), 6), 0..6);
    }

    #[test]
    fn full_line_count() {
        assert_eq!(full_lines_in_viewport(LH, 200.0), 9);
        assert_eq!(full_lines_in_viewport(LH, 5.0), 1);
    }

    #[test]
    fn reveal_keeps_three_lines_and_five_columns() {
        let vp = (800.0, 420.0); // 20 lines
        // Cursor below the viewport: scroll so that 3 lines remain below it.
        let (_, y) = reveal((0.0, 0.0), 30, 0.0, 8.0, LH, 8.0, vp);
        assert_eq!(y, 31.0 * LH + 3.0 * LH - 420.0);
        // Cursor above: 3 lines of context above.
        let (_, y) = reveal((0.0, 1000.0), 10, 0.0, 8.0, LH, 8.0, vp);
        assert_eq!(y, 10.0 * LH - 3.0 * LH);
        // Already comfortably visible: untouched.
        assert_eq!(reveal((0.0, 0.0), 8, 40.0, 8.0, LH, 8.0, vp), (0.0, 0.0));
        // Near the top, scroll clamps at zero.
        assert_eq!(reveal((0.0, 30.0), 1, 0.0, 8.0, LH, 8.0, vp).1, 0.0);
        // Horizontal: 5 columns of margin.
        let (x, _) = reveal((0.0, 0.0), 0, 1000.0, 8.0, LH, 8.0, vp);
        assert_eq!(x, 1000.0 + 8.0 + 40.0 - 800.0);
        let (x, _) = reveal((900.0, 0.0), 0, 500.0, 8.0, LH, 8.0, vp);
        assert_eq!(x, 500.0 - 40.0);
    }

    #[test]
    fn reveal_margins_shrink_in_tiny_viewports() {
        // 200px viewport = 9 lines: margin stays 3. 60px = 2 lines: margin 0, cursor still visible.
        let (_, y) = reveal((0.0, 0.0), 10, 0.0, 8.0, LH, 8.0, (400.0, 60.0));
        assert!(y <= 10.0 * LH && y + 60.0 >= 11.0 * LH);
        // 3 lines: margin 1.
        let (_, y) = reveal((0.0, 0.0), 10, 0.0, 8.0, LH, 8.0, (400.0, 63.0));
        assert_eq!(y, 11.0 * LH + LH - 63.0);
    }

    #[test]
    fn scroll_requests() {
        let vh = 420.0;
        assert_eq!(scroll_for_request(ScrollRequest::CursorTop, 10, 0.0, LH, vh), 210.0);
        assert_eq!(scroll_for_request(ScrollRequest::CursorCenter, 10, 0.0, LH, vh), 210.0 + 10.5 - 210.0);
        assert_eq!(scroll_for_request(ScrollRequest::CursorBottom, 30, 0.0, LH, vh), 31.0 * LH - vh);
        assert_eq!(scroll_for_request(ScrollRequest::Lines(-2), 0, 100.0, LH, vh), 100.0 - 2.0 * LH);
    }

    #[test]
    fn line_under_pointer() {
        assert_eq!(line_at_y(5.0, 0.0, LH, 10), 0);
        assert_eq!(line_at_y(5.0, 42.0, LH, 10), 2);
        assert_eq!(line_at_y(5000.0, 0.0, LH, 10), 9);
        assert_eq!(line_at_y(-50.0, 0.0, LH, 10), 0);
    }

    #[test]
    fn gutter_fits_largest_number() {
        let w3 = gutter_width(999, 8.0, 12.0, 4.0, 10.0);
        let w4 = gutter_width(1000, 8.0, 12.0, 4.0, 10.0);
        assert_eq!(w4 - w3, 8.0);
        // Never narrower than two digits.
        assert_eq!(gutter_width(1, 8.0, 12.0, 4.0, 10.0), 12.0 + 4.0 + 16.0 + 10.0);
    }

    #[test]
    fn thumb_round_trip() {
        let (top, h) = thumb_metrics(500.0, 1000.0, 400.0, 400.0, 24.0);
        assert!((h - 400.0 / 1400.0 * 400.0).abs() < 0.01);
        let back = scroll_for_thumb(top, h, 400.0, 1000.0);
        assert!((back - 500.0).abs() < 0.01);
        // Tiny viewport/content ratio keeps a grabbable thumb.
        assert_eq!(thumb_metrics(0.0, 1_000_000.0, 100.0, 400.0, 24.0).1, 24.0);
        // Nothing to scroll: thumb fills the track.
        assert_eq!(thumb_metrics(0.0, 0.0, 400.0, 400.0, 24.0), (0.0, 400.0));
    }

    #[test]
    fn autoscroll_grows_and_caps() {
        assert_eq!(autoscroll_step(0.0, LH), 0.0);
        assert!(autoscroll_step(10.0, LH) < autoscroll_step(50.0, LH));
        assert_eq!(autoscroll_step(10_000.0, LH), LH * 3.0);
    }

    #[test]
    fn popup_prefers_below() {
        let p = place_popup((100.0, 100.0, 21.0), (300.0, 200.0), (1000.0, 800.0), 2.0, 8.0);
        assert_eq!(p, Placement { x: 100.0, y: 123.0, above: false });
    }

    #[test]
    fn popup_flips_above_when_cramped() {
        let p = place_popup((100.0, 700.0, 21.0), (300.0, 200.0), (1000.0, 800.0), 2.0, 8.0);
        assert!(p.above);
        assert_eq!(p.y, 700.0 - 2.0 - 200.0);
    }

    #[test]
    fn popup_clamps_inside_window() {
        let p = place_popup((900.0, 100.0, 21.0), (300.0, 200.0), (1000.0, 800.0), 2.0, 8.0);
        assert_eq!(p.x, 1000.0 - 300.0 - 8.0);
        // Neither side fits: take the roomier one and stay on screen.
        let p = place_popup((10.0, 150.0, 21.0), (300.0, 400.0), (1000.0, 300.0), 2.0, 8.0);
        assert!(p.y >= 8.0);
        assert!(!p.above || p.y >= 8.0);
        // Left clamp.
        let p = place_popup((-50.0, 10.0, 21.0), (300.0, 100.0), (1000.0, 800.0), 2.0, 8.0);
        assert_eq!(p.x, 8.0);
    }
}
