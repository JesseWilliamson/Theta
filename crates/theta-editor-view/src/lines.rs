//! Shaped-line cache.
//!
//! Shaping is the expensive part of painting text. A line's shaped glyphs depend only on its
//! text, its highlight spans, the font and the syntax colours, so those are hashed into the key:
//! scrolling, selecting and cursor movement never re-shape, and an edit re-shapes only the
//! lines whose text or colouring changed (typing in a block comment re-colours the lines
//! below it, and only those).

use crate::display::{DisplayLine, TAB_WIDTH};
use gpui_kit::{Font, FontStyle, Hsla, Pixels, ShapedLine, SharedString, TextRun, WindowTextSystem};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use theta_editor::{HighlightKind, HighlightSpan};
use theta_ui::SyntaxColors;

pub struct LineEntry {
    pub display: DisplayLine,
    pub shaped: ShapedLine,
    /// Pixel width of the shaped text.
    pub width: Pixels,
}

impl LineEntry {
    /// x offset (from the start of the line) of source char column `col`.
    pub fn x_of_col(&self, col: usize) -> Pixels {
        self.shaped.x_for_index(self.display.byte_of_char(col))
    }

    /// Source char column under `x` (the glyph containing it), clamped to the line.
    pub fn col_at_x(&self, x: Pixels, nearest: bool) -> usize {
        if x <= Pixels::ZERO {
            return 0;
        }
        if x >= self.width {
            return self.display.char_count();
        }
        let byte = if nearest {
            self.shaped.closest_index_for_x(x)
        } else {
            self.shaped.index_for_x(x).unwrap_or(self.display.text.len())
        };
        self.display.char_of_byte(byte)
    }
}

/// Everything colour- and font-related that is baked into shaped runs.
#[derive(Clone)]
pub struct Style {
    pub font: Font,
    pub font_size: Pixels,
    pub colors: SyntaxColors,
    pub foreground: Hsla,
    /// Hash of the above, part of every cache key.
    pub epoch: u64,
}

impl Style {
    pub fn new(font: Font, font_size: Pixels, colors: SyntaxColors, foreground: Hsla) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        font.family.hash(&mut h);
        f32::from(font_size).to_bits().hash(&mut h);
        let mut color = |c: Hsla| {
            for v in [c.h, c.s, c.l, c.a] {
                v.to_bits().hash(&mut h);
            }
        };
        for c in [
            colors.keyword,
            colors.type_,
            colors.function,
            colors.string,
            colors.number,
            colors.comment,
            colors.operator,
            colors.punctuation,
            colors.identifier,
            colors.constant,
            colors.parameter,
            colors.error,
            foreground,
        ] {
            color(c);
        }
        colors.comment_italic.hash(&mut h);
        Style { font, font_size, colors, foreground, epoch: h.finish() }
    }

    fn color_of(&self, kind: HighlightKind) -> Hsla {
        let c = &self.colors;
        match kind {
            HighlightKind::Keyword => c.keyword,
            HighlightKind::Type => c.type_,
            HighlightKind::Function => c.function,
            HighlightKind::String => c.string,
            HighlightKind::Number => c.number,
            HighlightKind::Comment => c.comment,
            HighlightKind::Operator => c.operator,
            HighlightKind::Punctuation => c.punctuation,
            HighlightKind::Identifier => self.foreground,
            HighlightKind::Constant => c.constant,
            HighlightKind::Parameter => c.parameter,
            HighlightKind::Error => c.error,
        }
    }

    fn run(&self, len: usize, kind: Option<HighlightKind>) -> TextRun {
        let mut font = self.font.clone();
        if kind == Some(HighlightKind::Comment) && self.colors.comment_italic {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color: kind.map_or(self.foreground, |k| self.color_of(k)),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }
}

/// Text runs covering `display.text` exactly. Spans are in source chars; gaps get the
/// foreground colour. Spans may arrive unsorted/overlapping from a buggy lexer: they are
/// clamped so the run lengths always sum to the text length (a mismatch would panic in the shaper).
pub fn build_runs(display: &DisplayLine, spans: &[HighlightSpan], style: &Style) -> Vec<TextRun> {
    let total = display.text.len();
    if total == 0 {
        return Vec::new();
    }
    let mut runs: Vec<TextRun> = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0usize;
    for span in spans {
        let start = display.byte_of_char(span.range.start).max(at);
        let end = display.byte_of_char(span.range.end).min(total);
        if end <= start {
            continue;
        }
        if start > at {
            runs.push(style.run(start - at, None));
        }
        runs.push(style.run(end - start, Some(span.kind)));
        at = end;
    }
    if at < total {
        runs.push(style.run(total - at, None));
    }
    // Merge neighbours that look identical (identifier gaps next to Identifier spans).
    let mut merged: Vec<TextRun> = Vec::with_capacity(runs.len());
    for r in runs {
        match merged.last_mut() {
            Some(last) if last.color == r.color && last.font == r.font => last.len += r.len,
            _ => merged.push(r),
        }
    }
    merged
}

struct Slot {
    entry: Rc<LineEntry>,
    used: u64,
}

#[derive(Default)]
pub struct LineCache {
    map: HashMap<u64, Slot>,
    frame: u64,
    pub shaped_this_frame: usize,
    pub hits_this_frame: usize,
}

const MAX_ENTRIES: usize = 4096;

impl LineCache {
    /// Call once per frame before the first lookup: drives eviction and the per-frame counters.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.shaped_this_frame = 0;
        self.hits_this_frame = 0;
        if self.map.len() > MAX_ENTRIES {
            let cutoff = self.frame.saturating_sub(2);
            self.map.retain(|_, s| s.used >= cutoff);
            if self.map.len() > MAX_ENTRIES {
                self.map.clear();
            }
        }
    }

    pub fn key(text: &str, spans: &[HighlightSpan], epoch: u64) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        epoch.hash(&mut h);
        text.hash(&mut h);
        for s in spans {
            s.range.start.hash(&mut h);
            s.range.end.hash(&mut h);
            s.kind.hash(&mut h);
        }
        h.finish()
    }

    pub fn get(
        &mut self,
        text_system: &WindowTextSystem,
        src: &str,
        spans: &[HighlightSpan],
        style: &Style,
    ) -> Rc<LineEntry> {
        let key = Self::key(src, spans, style.epoch);
        if let Some(slot) = self.map.get_mut(&key) {
            slot.used = self.frame;
            self.hits_this_frame += 1;
            return slot.entry.clone();
        }
        self.shaped_this_frame += 1;
        let display = DisplayLine::new(src, TAB_WIDTH);
        let runs = build_runs(&display, spans, style);
        let shaped = text_system.shape_line(
            SharedString::from(display.text.to_string()),
            style.font_size,
            &runs,
            None,
        );
        let width = shaped.width();
        let entry = Rc::new(LineEntry { display, shaped, width });
        self.map.insert(key, Slot { entry: entry.clone(), used: self.frame });
        entry
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{font, hsla, px};

    fn style() -> Style {
        let c = hsla(0.5, 0.5, 0.5, 1.0);
        let colors = SyntaxColors {
            keyword: hsla(0.0, 1.0, 0.5, 1.0),
            type_: c,
            function: c,
            string: hsla(0.3, 1.0, 0.5, 1.0),
            number: c,
            comment: hsla(0.6, 0.2, 0.5, 1.0),
            operator: c,
            punctuation: c,
            identifier: c,
            constant: c,
            parameter: c,
            error: c,
            comment_italic: true,
        };
        Style::new(font("Lilex"), px(14.), colors, hsla(0.0, 0.0, 1.0, 1.0))
    }

    fn span(r: std::ops::Range<usize>, kind: HighlightKind) -> HighlightSpan {
        HighlightSpan { range: r, kind }
    }

    #[test]
    fn runs_cover_text_exactly() {
        let st = style();
        let d = DisplayLine::new("select 'é' -- hi", 4);
        let spans = [
            span(0..6, HighlightKind::Keyword),
            span(7..10, HighlightKind::String),
            span(11..16, HighlightKind::Comment),
        ];
        let runs = build_runs(&d, &spans, &st);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), d.text.len());
        assert_eq!(runs[0].len, 6);
        assert_eq!(runs[0].color, st.colors.keyword);
        // The comment run is italic when the theme says so.
        assert_eq!(runs.last().unwrap().font.style, FontStyle::Italic);
    }

    #[test]
    fn overlapping_or_out_of_range_spans_cannot_break_the_sum() {
        let st = style();
        let d = DisplayLine::new("abcdef", 4);
        let spans = [
            span(0..4, HighlightKind::Keyword),
            span(2..5, HighlightKind::String), // overlaps: clamped to start at 4
            span(5..99, HighlightKind::Number), // runs past the end
            span(3..3, HighlightKind::Comment), // empty
        ];
        let runs = build_runs(&d, &spans, &st);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 6);
    }

    #[test]
    fn empty_line_has_no_runs() {
        assert!(build_runs(&DisplayLine::new("", 4), &[], &style()).is_empty());
    }

    #[test]
    fn tabs_keep_byte_accounting() {
        let st = style();
        let d = DisplayLine::new("\tselect", 4);
        let runs = build_runs(&d, &[span(1..7, HighlightKind::Keyword)], &st);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), d.text.len());
        assert_eq!(runs[0].len, 4); // the expanded tab is the unstyled gap
    }

    #[test]
    fn key_changes_with_text_spans_and_epoch() {
        let a = LineCache::key("select 1", &[span(0..6, HighlightKind::Keyword)], 1);
        assert_eq!(a, LineCache::key("select 1", &[span(0..6, HighlightKind::Keyword)], 1));
        assert_ne!(a, LineCache::key("select 2", &[span(0..6, HighlightKind::Keyword)], 1));
        assert_ne!(a, LineCache::key("select 1", &[span(0..6, HighlightKind::Type)], 1));
        assert_ne!(a, LineCache::key("select 1", &[span(0..6, HighlightKind::Keyword)], 2));
    }

    #[test]
    fn epoch_depends_on_colours_and_font() {
        let a = style();
        let mut b = style();
        b.colors.keyword = hsla(0.9, 1.0, 0.5, 1.0);
        let b = Style::new(b.font.clone(), b.font_size, b.colors, b.foreground);
        assert_ne!(a.epoch, b.epoch);
        let c = Style::new(font("Other"), px(14.), a.colors, a.foreground);
        assert_ne!(a.epoch, c.epoch);
    }
}
