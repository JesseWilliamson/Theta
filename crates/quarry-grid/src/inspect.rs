//! Content model of the cell inspector: what to show for a value. Pure.

use crate::format::{format_count, raw_text};
use quarry_core::Value;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonTok {
    Key,
    Str,
    Number,
    Literal,
    Punct,
}

/// Longest EWKT shown; geometry text can be megabytes.
pub const MAX_EWKT_CHARS: usize = 20_000;
/// Longest text/JSON body rendered; beyond this the host is told to export.
pub const MAX_BODY_CHARS: usize = 200_000;
pub const MAX_HEX_BYTES: usize = 4096;

/// Highlight spans over a pretty-printed JSON text.
pub type JsonSpans = Vec<(Range<usize>, JsonTok)>;

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Null,
    Text(String),
    Json { text: String, spans: JsonSpans },
    Hex(String),
    Geometry { facts: Vec<(&'static str, String)>, ewkt: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inspect {
    /// "12 characters · 12 bytes", "256 bytes", ...
    pub stats: String,
    pub body: Body,
    /// A truncation / parse note shown under the body.
    pub note: Option<String>,
}

pub fn inspect_value(value: &Value) -> Inspect {
    match value {
        Value::Null => Inspect { stats: "NULL".into(), body: Body::Null, note: None },
        Value::Bytes(b) => Inspect {
            stats: format!("{} bytes", format_count(b.len())),
            body: Body::Hex(hex_dump(b, MAX_HEX_BYTES)),
            note: (b.len() > MAX_HEX_BYTES).then(|| {
                format!(
                    "Showing the first {} of {} bytes.",
                    format_count(MAX_HEX_BYTES),
                    format_count(b.len())
                )
            }),
        },
        Value::Geometry(b) => inspect_geometry(b),
        Value::Json(s) => {
            let (text, note) = cap(s);
            let stats = text_stats(s);
            match json_pretty(&text) {
                Some((pretty, spans)) => Inspect { stats, body: Body::Json { text: pretty, spans }, note },
                None => Inspect {
                    stats,
                    body: Body::Text(text),
                    note: note.or_else(|| Some("Not valid JSON; shown as plain text.".into())),
                },
            }
        }
        other => {
            let raw = raw_text(other);
            let stats = if matches!(other, Value::Text(_)) { text_stats(&raw) } else { String::new() };
            let (text, note) = cap(&raw);
            Inspect { stats, body: Body::Text(text), note }
        }
    }
}

fn text_stats(s: &str) -> String {
    format!("{} characters · {} bytes", format_count(s.chars().count()), format_count(s.len()))
}

fn cap(s: &str) -> (String, Option<String>) {
    match s.char_indices().nth(MAX_BODY_CHARS) {
        Some((i, _)) => (
            s[..i].to_string(),
            Some(format!(
                "Showing the first {} characters; export the cell to see all of it.",
                format_count(MAX_BODY_CHARS)
            )),
        ),
        None => (s.to_string(), None),
    }
}

fn inspect_geometry(bytes: &[u8]) -> Inspect {
    let stats = format!("{} bytes", format_count(bytes.len()));
    let Ok(feature) = quarry_geo::parse_wkb(bytes) else {
        return Inspect {
            stats,
            body: Body::Text("The bytes could not be decoded as WKB/EWKB.".into()),
            note: None,
        };
    };
    let g = &feature.geometry;
    let mut facts = vec![
        ("Type", g.type_name().to_string()),
        ("SRID", feature.srid.map_or_else(|| "not set".to_string(), |s| s.to_string())),
        ("Vertices", format_count(g.coord_count())),
        ("Summary", quarry_geo::describe(&feature)),
    ];
    if let Some(b) = g.bbox().filter(|b| b.min != b.max) {
        facts
            .push(("Bounds", format!("{:.6}, {:.6} → {:.6}, {:.6}", b.min[0], b.min[1], b.max[0], b.max[1])));
    }
    let full = quarry_geo::to_ewkt(&feature, None);
    let (ewkt, note) = match full.char_indices().nth(MAX_EWKT_CHARS) {
        Some((i, _)) => (
            format!("{}…", &full[..i]),
            Some(format!(
                "EWKT truncated: showing {} of {} characters.",
                format_count(MAX_EWKT_CHARS),
                format_count(full.chars().count())
            )),
        ),
        None => (full, None),
    };
    Inspect { stats, body: Body::Geometry { facts, ewkt }, note }
}

/// `offset  hex bytes  |ascii|`, eight bytes per line (the pane is narrow).
pub fn hex_dump(bytes: &[u8], limit: usize) -> String {
    let shown = &bytes[..bytes.len().min(limit)];
    let mut out = String::new();
    for (i, chunk) in shown.chunks(8).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("{:08x}  ", i * 8));
        for j in 0..8 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
        }
        out.push_str(" |");
        out.extend(chunk.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
        out.push('|');
    }
    out
}

/// Re-indent JSON text without reordering keys or touching number/string
/// text, and report highlight spans over the result. `None` when `text` is
/// not valid JSON.
pub fn json_pretty(text: &str) -> Option<(String, JsonSpans)> {
    serde_json::from_str::<serde_json::Value>(text).ok()?;
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() * 2);
    let mut spans: JsonSpans = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        for _ in 0..depth {
            out.push_str("  ");
        }
    };
    let push = |out: &mut String, spans: &mut JsonSpans, s: &str, tok: JsonTok| {
        let start = out.len();
        out.push_str(s);
        spans.push((start..out.len(), tok));
    };
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b'{' | b'[' => {
                let close = if b[i] == b'{' { b'}' } else { b']' };
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == close {
                    push(&mut out, &mut spans, &text[i..=i], JsonTok::Punct);
                    push(&mut out, &mut spans, &text[j..=j], JsonTok::Punct);
                    i = j + 1;
                } else {
                    push(&mut out, &mut spans, &text[i..=i], JsonTok::Punct);
                    depth += 1;
                    newline(&mut out, depth);
                    i += 1;
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                push(&mut out, &mut spans, &text[i..=i], JsonTok::Punct);
                i += 1;
            }
            b',' => {
                push(&mut out, &mut spans, ",", JsonTok::Punct);
                newline(&mut out, depth);
                i += 1;
            }
            b':' => {
                push(&mut out, &mut spans, ": ", JsonTok::Punct);
                i += 1;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(b.len());
                let mut k = end;
                while k < b.len() && b[k].is_ascii_whitespace() {
                    k += 1;
                }
                let tok = if k < b.len() && b[k] == b':' { JsonTok::Key } else { JsonTok::Str };
                push(&mut out, &mut spans, &text[i..end], tok);
                i = end;
            }
            _ => {
                let mut j = i;
                while j < b.len() && !matches!(b[j], b',' | b'}' | b']' | b':') && !b[j].is_ascii_whitespace()
                {
                    j += 1;
                }
                let word = &text[i..j];
                let tok = if matches!(word, "true" | "false" | "null") {
                    JsonTok::Literal
                } else {
                    JsonTok::Number
                };
                push(&mut out, &mut spans, word, tok);
                i = j;
            }
        }
    }
    Some((out, spans))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_is_indented_and_order_preserving() {
        let (pretty, spans) = json_pretty(r#"{"b":1,"a":[true,null,"x"],"e":{},"s":[]}"#).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"b\": 1,\n  \"a\": [\n    true,\n    null,\n    \"x\"\n  ],\n  \"e\": {},\n  \"s\": []\n}"
        );
        let kinds = |tok| {
            spans.iter().filter(|(_, t)| *t == tok).map(|(r, _)| &pretty[r.clone()]).collect::<Vec<_>>()
        };
        assert_eq!(kinds(JsonTok::Key), ["\"b\"", "\"a\"", "\"e\"", "\"s\""]);
        assert_eq!(kinds(JsonTok::Str), ["\"x\""]);
        assert_eq!(kinds(JsonTok::Literal), ["true", "null"]);
        assert_eq!(kinds(JsonTok::Number), ["1"]);
    }

    #[test]
    fn json_keeps_number_text_and_escapes() {
        let (pretty, _) = json_pretty(r#"[1.50, 1e5, "a\"b:c"]"#).unwrap();
        assert!(pretty.contains("1.50"));
        assert!(pretty.contains("1e5"));
        assert!(pretty.contains(r#""a\"b:c""#));
    }

    #[test]
    fn invalid_json_is_none() {
        assert!(json_pretty("{oops").is_none());
        assert!(json_pretty("").is_none());
    }

    #[test]
    fn scalar_json() {
        assert_eq!(json_pretty("  42 ").unwrap().0, "42");
        assert_eq!(json_pretty("\"hi\"").unwrap().0, "\"hi\"");
    }

    #[test]
    fn hex_dump_layout() {
        let d = hex_dump(b"Hello, world!\x00\xff", 4096);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "00000000  48 65 6c 6c 6f 2c 20 77  |Hello, w|");
        assert_eq!(lines[1], "00000008  6f 72 6c 64 21 00 ff     |orld!..|");
        assert_eq!(hex_dump(&[1; 100], 16).lines().count(), 2);
    }

    #[test]
    fn inspect_text_stats() {
        let i = inspect_value(&Value::Text("héllo".into()));
        assert_eq!(i.stats, "5 characters · 6 bytes");
        assert_eq!(i.body, Body::Text("héllo".into()));
        assert!(i.note.is_none());
    }

    #[test]
    fn inspect_truncates_huge_text() {
        let i = inspect_value(&Value::Text("x".repeat(MAX_BODY_CHARS + 10)));
        let Body::Text(t) = i.body else { panic!() };
        assert_eq!(t.chars().count(), MAX_BODY_CHARS);
        assert!(i.note.unwrap().contains("first 200,000"));
    }

    #[test]
    fn inspect_bytes_and_null() {
        let i = inspect_value(&Value::Bytes(vec![0; 5000]));
        assert_eq!(i.stats, "5,000 bytes");
        assert!(i.note.unwrap().contains("4,096 of 5,000"));
        assert_eq!(inspect_value(&Value::Null).body, Body::Null);
    }

    #[test]
    fn inspect_invalid_json_falls_back_to_text() {
        let i = inspect_value(&Value::Json("{nope".into()));
        assert!(matches!(i.body, Body::Text(_)));
        assert!(i.note.is_some());
    }

    #[test]
    fn inspect_geometry_facts() {
        let f = quarry_geo::parse_wkt("SRID=4326;POLYGON((0 0,0 1,1 1,1 0,0 0))").unwrap();
        let i = inspect_value(&Value::Geometry(quarry_geo::to_ewkb(&f)));
        let Body::Geometry { facts, ewkt } = i.body else { panic!() };
        assert_eq!(facts[0], ("Type", "Polygon".to_string()));
        assert_eq!(facts[1], ("SRID", "4326".to_string()));
        assert_eq!(facts[2], ("Vertices", "5".to_string()));
        assert!(ewkt.starts_with("SRID=4326;POLYGON"));
        assert!(i.note.is_none());
    }

    #[test]
    fn inspect_geometry_truncates_ewkt() {
        let coords: Vec<String> = (0..4000).map(|i| format!("{i}.123456 {i}.654321")).collect();
        let wkt = format!("LINESTRING({})", coords.join(","));
        let f = quarry_geo::parse_wkt(&wkt).unwrap();
        let i = inspect_value(&Value::Geometry(quarry_geo::to_ewkb(&f)));
        let Body::Geometry { ewkt, .. } = i.body else { panic!() };
        assert!(ewkt.chars().count() <= MAX_EWKT_CHARS + 1);
        assert!(i.note.unwrap().contains("truncated"));
    }
}
