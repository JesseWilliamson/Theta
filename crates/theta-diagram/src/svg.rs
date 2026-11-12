//! A standalone SVG of a laid-out diagram, written by hand (no dependency): the file has to look right in
//! any viewer, so it carries its own colours ([`Palette`]) and uses only rectangles, lines, polylines and text.

use crate::layout::{BoxLayout, EdgeLayout, Layout, Metrics, clip, end_marks, marker, more_text};
use crate::model::Graph;
use std::fmt::Write as _;

/// Colours as CSS strings. The application fills it from the current theme, so an exported file matches the
/// window it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub background: String,
    pub box_fill: String,
    pub header_fill: String,
    pub border: String,
    pub text: String,
    pub muted: String,
    pub edge: String,
    pub key: String,
}

impl Palette {
    pub fn light() -> Palette {
        Palette {
            background: "#ffffff".into(),
            box_fill: "#f8f9fb".into(),
            header_fill: "#e3e8f0".into(),
            border: "#9aa5b8".into(),
            text: "#1f2430".into(),
            muted: "#6b7385".into(),
            edge: "#5b6b8c".into(),
            key: "#b45f06".into(),
        }
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            // XML 1.0 cannot carry most control characters at all.
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => o.push(c),
        }
    }
    o
}

fn num(v: f32) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

pub fn render(g: &Graph, l: &Layout, m: &Metrics, p: &Palette, title: &str) -> String {
    let (w, h) = (l.width.max(1.0), l.height.max(1.0));
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}" font-family="ui-monospace, 'SF Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace" font-size="12">"#,
        num(w),
        num(h),
        num(w),
        num(h)
    );
    let _ = writeln!(s, "<title>{}</title>", esc(title));
    let _ = writeln!(s, r#"<rect width="100%" height="100%" fill="{}"/>"#, esc(&p.background));
    for e in &l.edges {
        edge(&mut s, g, e, p);
    }
    for b in &l.boxes {
        table(&mut s, g, b, m, p);
    }
    s.push_str("</svg>\n");
    s
}

fn edge(s: &mut String, g: &Graph, e: &EdgeLayout, p: &Palette) {
    let ge = &g.edges[e.edge];
    let pts: Vec<String> = e.points.iter().map(|(x, y)| format!("{},{}", num(*x), num(*y))).collect();
    // A foreign key whose columns allow NULL is dashed: a row need not have a parent.
    let dash = if ge.optional { r#" stroke-dasharray="6 4""# } else { "" };
    let _ = writeln!(
        s,
        r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="1.5" stroke-linejoin="round"{}><title>{}</title></polyline>"#,
        pts.join(" "),
        esc(&p.edge),
        dash,
        esc(&format!(
            "{}: {}({}) → {}({})",
            ge.name,
            g.label(ge.from),
            ge.from_cols.join(", "),
            g.label(ge.to),
            ge.to_cols.join(", ")
        ))
    );
    for ((x1, y1), (x2, y2)) in end_marks(e, ge.unique) {
        let _ = writeln!(
            s,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="1.5"/>"#,
            num(x1),
            num(y1),
            num(x2),
            num(y2),
            esc(&p.edge)
        );
    }
    for br in [&e.start_bracket, &e.end_bracket].into_iter().flatten() {
        let (y0, y1) = br.y_range();
        let _ = writeln!(
            s,
            r#"<polyline points="{},{} {},{} {},{} {},{}" fill="none" stroke="{}" stroke-width="1.5"/>"#,
            num(br.x_edge),
            num(y0),
            num(br.x_out),
            num(y0),
            num(br.x_out),
            num(y1),
            num(br.x_edge),
            num(y1),
            esc(&p.edge)
        );
        for t in &br.ticks {
            let _ = writeln!(
                s,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="1.5"/>"#,
                num(br.x_edge),
                num(*t),
                num(br.x_out),
                num(*t),
                esc(&p.edge)
            );
        }
    }
}

fn table(s: &mut String, g: &Graph, b: &BoxLayout, m: &Metrics, p: &Palette) {
    let node = &g.nodes[b.node];
    let _ = writeln!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="4" fill="{}" stroke="{}" stroke-width="1.2"/>"#,
        num(b.x),
        num(b.y),
        num(b.w),
        num(b.h),
        esc(&p.box_fill),
        esc(&p.border)
    );
    // The header's lower corners are square, so it is a path rather than a second rounded rectangle.
    let (x0, y0, x1, y1) = (b.x, b.y, b.x + b.w, b.y + m.header_h);
    let _ = writeln!(
        s,
        r#"<path d="M{x0},{ya} a4,4 0 0 1 4,-4 H{xr} a4,4 0 0 1 4,4 V{y1} H{x0} Z" fill="{}" stroke="{}" stroke-width="1.2"/>"#,
        esc(&p.header_fill),
        esc(&p.border),
        x0 = num(x0),
        ya = num(y0 + 4.0),
        xr = num(x1 - 4.0),
        y1 = num(y1),
    );
    let _ = writeln!(
        s,
        r#"<text x="{}" y="{}" font-weight="bold" fill="{}">{}</text>"#,
        num(b.x + m.pad_x),
        num(b.y + m.header_h / 2.0 + 4.5),
        esc(&p.text),
        esc(&clip(&g.label(b.node), m.max_name_chars + 8))
    );
    let marker_w = crate::layout::MARKER_CHARS as f32 * m.char_w;
    for r in &b.rows {
        let c = &node.columns[r.column];
        let base = r.y + m.row_h / 2.0 + 4.0;
        let mk = marker(c);
        if !mk.is_empty() {
            let _ = writeln!(
                s,
                r#"<text x="{}" y="{}" font-size="10" font-weight="bold" fill="{}">{}</text>"#,
                num(b.x + m.pad_x),
                num(base),
                esc(&p.key),
                mk
            );
        }
        let _ = writeln!(
            s,
            r#"<text x="{}" y="{}" fill="{}"{}>{}</text>"#,
            num(b.x + m.pad_x + marker_w),
            num(base),
            esc(&p.text),
            if c.pk { r#" font-weight="bold""# } else { "" },
            esc(&clip(&c.name, m.max_name_chars))
        );
        let _ = writeln!(
            s,
            r#"<text x="{}" y="{}" text-anchor="end" fill="{}">{}</text>"#,
            num(b.x + b.w - m.pad_x),
            num(base),
            esc(&p.muted),
            esc(&clip(&c.data_type, m.max_type_chars))
        );
    }
    if b.hidden > 0 {
        let y = b.y + m.header_h + b.rows.len() as f32 * m.row_h + m.row_h / 2.0 + 4.0;
        let _ = writeln!(
            s,
            r#"<text x="{}" y="{}" fill="{}" font-style="italic">{}</text>"#,
            num(b.x + m.pad_x + marker_w),
            num(y),
            esc(&p.muted),
            esc(&more_text(b.hidden))
        );
    }
}
