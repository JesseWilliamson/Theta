//! Number, duration and cell-text formatting. Pure: no GPUI types, so the
//! rules that decide what the grid shows are unit-testable.

use std::time::Duration;
use theta_core::Value;

/// `1234567` -> `1,234,567`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    group_digits(&digits)
}

fn group_digits(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn format_count(n: usize) -> String {
    thousands(n as u64)
}

/// `1234.5` -> `1,234.5`; at most four decimals, trailing zeros trimmed.
pub fn format_number(v: f64) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let s = format!("{:.4}", v.abs());
    let s = s.trim_end_matches('0').trim_end_matches('.');
    let (int, frac) = s.split_once('.').map_or((s, None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    if v < 0.0 && s != "0" {
        out.push('-');
    }
    out.push_str(&group_digits(int));
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// `42 ms`, `1.24 s`, `2 min 05 s`. Sub-millisecond runs say `<1 ms` rather
/// than a misleading `0 ms`.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "<1 ms".into()
    } else if ms < 1000.0 {
        format!("{} ms", ms.round() as u64)
    } else if d.as_secs() < 60 {
        format!("{:.2} s", d.as_secs_f64())
    } else {
        format!("{} min {:02} s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// How a cell is styled; decided by the value, not the column type, because a
/// driver may legitimately return `NULL` in any column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Null,
    EmptyText,
    Bool(bool),
    Number,
    Text,
    Json,
    Bytes,
    Geometry,
    Temporal,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellText {
    pub text: String,
    pub kind: CellKind,
}

/// Longest text kept for a grid cell: far wider than the 400px column cap, so
/// ellipsis still shows, but a 5 MB text value never becomes a 5 MB string
/// per cell.
pub const MAX_CELL_CHARS: usize = 400;

/// Collapse a value's text to one line: newlines become `↵`, tabs and other
/// control characters a space, and the result is cut at [`MAX_CELL_CHARS`].
pub fn single_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_CELL_CHARS + 4));
    let mut count = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if count >= MAX_CELL_CHARS {
            out.push('…');
            return out;
        }
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('↵');
            }
            '\n' => out.push('↵'),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
        count += 1;
    }
    out
}

pub fn hex_preview(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(2 + bytes.len().min(32) * 2 + 16);
    out.push_str("\\x");
    for b in bytes.iter().take(32) {
        out.push_str(&format!("{b:02x}"));
    }
    if bytes.len() > 32 {
        out.push_str(&format!("… ({} bytes)", bytes.len()));
    }
    out
}

/// Text for one cell plus the styling class.
pub fn cell_text(value: &Value) -> CellText {
    let (text, kind) = match value {
        Value::Null => (String::new(), CellKind::Null),
        Value::Bool(b) => (b.to_string(), CellKind::Bool(*b)),
        Value::Int(_) | Value::UInt(_) | Value::Float(_) | Value::Decimal(_) => {
            (value.display(), CellKind::Number)
        }
        Value::Text(s) if s.is_empty() => (String::new(), CellKind::EmptyText),
        Value::Text(s) | Value::Unknown(s) | Value::Uuid(s) | Value::Interval(s) => {
            (single_line(s), CellKind::Text)
        }
        Value::Json(s) => (single_line(s), CellKind::Json),
        Value::Date(s) | Value::Time(s) | Value::Timestamp(s) | Value::TimestampTz(s) => {
            (single_line(s), CellKind::Temporal)
        }
        Value::Bytes(b) => (hex_preview(b), CellKind::Bytes),
        Value::Array(_) => (single_line(&value.display()), CellKind::Text),
        Value::Geometry(b) => (
            match theta_geo::parse_wkb(b) {
                Ok(f) => theta_geo::describe(&f),
                Err(_) => format!("<invalid geometry, {} bytes>", b.len()),
            },
            CellKind::Geometry,
        ),
    };
    CellText { text, kind }
}

/// Full-fidelity text for copy/export/edit prefill (no truncation, real newlines).
pub fn raw_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bytes(b) => {
            let mut out = String::with_capacity(2 + b.len() * 2);
            out.push_str("\\x");
            for byte in b {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        }
        Value::Geometry(b) => match theta_geo::parse_wkb(b) {
            Ok(f) => theta_geo::to_ewkt(&f, None),
            Err(_) => format!("<invalid geometry, {} bytes>", b.len()),
        },
        other => other.display(),
    }
}

/// Approximate on-screen width in characters, used for column auto-fit.
pub fn display_chars(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Text(s) if s.is_empty() => 2,
        _ => cell_text(value).text.chars().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_grouping() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(format_count(100_000), "100,000");
    }

    #[test]
    fn number_formatting() {
        assert_eq!(format_number(1234.5), "1,234.5");
        assert_eq!(format_number(-1234567.0), "-1,234,567");
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(-0.00001), "0");
        assert_eq!(format_number(411.5), "411.5");
        assert_eq!(format_number(f64::INFINITY), "inf");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(Duration::from_micros(300)), "<1 ms");
        assert_eq!(format_duration(Duration::from_millis(42)), "42 ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999 ms");
        assert_eq!(format_duration(Duration::from_millis(1240)), "1.24 s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2 min 05 s");
    }

    #[test]
    fn single_line_collapses_newlines_and_truncates() {
        assert_eq!(single_line("a\nb"), "a↵b");
        assert_eq!(single_line("a\r\nb"), "a↵b");
        assert_eq!(single_line("a\tb"), "a b");
        let long = "x".repeat(1000);
        let s = single_line(&long);
        assert_eq!(s.chars().count(), MAX_CELL_CHARS + 1);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn cell_kinds() {
        assert_eq!(cell_text(&Value::Null).kind, CellKind::Null);
        assert_eq!(cell_text(&Value::Text(String::new())).kind, CellKind::EmptyText);
        assert_eq!(cell_text(&Value::Bool(true)).kind, CellKind::Bool(true));
        assert_eq!(cell_text(&Value::Decimal("1.50".into())).text, "1.50");
        assert_eq!(cell_text(&Value::Json("{\"a\":1}".into())).kind, CellKind::Json);
        assert_eq!(cell_text(&Value::Bytes(vec![0xde, 0xad])).text, "\\xdead");
        assert_eq!(cell_text(&Value::Geometry(vec![1, 2])).kind, CellKind::Geometry);
        assert!(cell_text(&Value::Geometry(vec![1, 2])).text.contains("invalid"));
    }

    #[test]
    fn geometry_cells_use_describe() {
        let f = theta_geo::parse_wkt("SRID=4326;POINT(1 2)").unwrap();
        let t = cell_text(&Value::Geometry(theta_geo::to_ewkb(&f)));
        assert_eq!(t.text, "Point (1, 2) · SRID 4326");
    }

    #[test]
    fn raw_text_is_not_truncated() {
        let long = "x\ny".repeat(1000);
        assert_eq!(raw_text(&Value::Text(long.clone())), long);
        let bytes = vec![0u8; 100];
        assert_eq!(raw_text(&Value::Bytes(bytes)).len(), 2 + 200);
    }
}
