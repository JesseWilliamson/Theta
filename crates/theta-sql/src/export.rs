//! Result-set export: CSV (RFC 4180), TSV, JSON, Markdown and SQL INSERTs.

use crate::generate::sql_literal;
use crate::keywords::quote_if_needed;
use theta_core::{DatabaseKind, ResultSet, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Markdown,
    SqlInserts { table: String },
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Plain-text form of a cell for the text formats. Unlike `Value::display` this
/// never truncates: an export must contain every byte.
fn cell_text(v: &Value) -> String {
    match v {
        Value::Bytes(b) => format!("\\x{}", hex(b)),
        Value::Geometry(b) => hex(b),
        Value::Array(items) => {
            let inner: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Value::Null => "NULL".to_string(),
                    Value::Text(s) | Value::Unknown(s) => {
                        let needs = s.is_empty()
                            || s.eq_ignore_ascii_case("null")
                            || s.chars()
                                .any(|c| matches!(c, ',' | '"' | '\\' | '{' | '}') || c.is_whitespace());
                        if needs {
                            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                        } else {
                            s.clone()
                        }
                    }
                    other => cell_text(other),
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        other => other.display(),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn csv(result: &ResultSet) -> String {
    let mut out = String::new();
    out.push_str(&result.columns.iter().map(|c| csv_field(&c.name)).collect::<Vec<_>>().join(","));
    out.push_str("\r\n");
    for row in &result.rows {
        let fields: Vec<String> = row
            .iter()
            .map(|v| match v {
                // NULL is an empty field; an empty string is `""` so the two stay distinguishable.
                Value::Null => String::new(),
                Value::Text(s) if s.is_empty() => "\"\"".to_string(),
                other => csv_field(&cell_text(other)),
            })
            .collect();
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

fn tsv_field(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn tsv(result: &ResultSet) -> String {
    let mut out = result.columns.iter().map(|c| tsv_field(&c.name)).collect::<Vec<_>>().join("\t");
    out.push('\n');
    for row in &result.rows {
        out.push_str(&row.iter().map(|v| tsv_field(&cell_text(v))).collect::<Vec<_>>().join("\t"));
        out.push('\n');
    }
    out
}

pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn is_json_number(s: &str) -> bool {
    // Strict JSON grammar, so exact decimals keep every digit without a float round trip.
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start || (b[int_start] == b'0' && i - int_start > 1) {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == f {
            return false;
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        i += 1;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let e = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e {
            return false;
        }
    }
    i == b.len()
}

fn json_value(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) if f.is_finite() => f.to_string(),
        Value::Decimal(s) if is_json_number(s) => s.clone(),
        Value::Json(s) if serde_json::from_str::<serde_json::Value>(s).is_ok() => s.clone(),
        Value::Array(items) => format!("[{}]", items.iter().map(json_value).collect::<Vec<_>>().join(", ")),
        other => json_string(&cell_text(other)),
    }
}

fn json(result: &ResultSet) -> String {
    if result.rows.is_empty() {
        return "[]\n".into();
    }
    let rows: Vec<String> = result
        .rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = result
                .columns
                .iter()
                .zip(row)
                .map(|(c, v)| format!("    {}: {}", json_string(&c.name), json_value(v)))
                .collect();
            format!("  {{\n{}\n  }}", fields.join(",\n"))
        })
        .collect();
    format!("[\n{}\n]\n", rows.join(",\n"))
}

fn md_cell(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        other => cell_text(other).replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>"),
    }
}

fn markdown(result: &ResultSet) -> String {
    let n = result.columns.len();
    let header: Vec<String> = result.columns.iter().map(|c| c.name.replace('|', "\\|")).collect();
    let body: Vec<Vec<String>> = result.rows.iter().map(|r| r.iter().map(md_cell).collect()).collect();
    let numeric: Vec<bool> = (0..n)
        .map(|i| {
            let vals: Vec<&Value> =
                result.rows.iter().filter_map(|r| r.get(i)).filter(|v| !v.is_null()).collect();
            !vals.is_empty() && vals.iter().all(|v| v.is_numeric())
        })
        .collect();
    let widths: Vec<usize> = (0..n)
        .map(|i| {
            let mut w = header[i].chars().count().max(3);
            for r in &body {
                w = w.max(r.get(i).map_or(0, |c| c.chars().count()));
            }
            w
        })
        .collect();
    let pad = |s: &str, w: usize, right: bool| {
        let fill = " ".repeat(w.saturating_sub(s.chars().count()));
        if right { format!("{fill}{s}") } else { format!("{s}{fill}") }
    };
    let mut out = String::new();
    let line = |cells: Vec<String>| format!("| {} |\n", cells.join(" | "));
    out.push_str(&line((0..n).map(|i| pad(&header[i], widths[i], false)).collect()));
    out.push_str(&line(
        (0..n)
            .map(
                |i| {
                    if numeric[i] { format!("{}:", "-".repeat(widths[i] - 1)) } else { "-".repeat(widths[i]) }
                },
            )
            .collect(),
    ));
    for r in &body {
        out.push_str(&line(
            (0..n).map(|i| pad(r.get(i).map_or("", |s| s.as_str()), widths[i], numeric[i])).collect(),
        ));
    }
    out
}

fn sql_inserts(result: &ResultSet, table: &str, kind: DatabaseKind) -> String {
    let target = if table.contains('.') && !table.contains(['"', '`', '[']) {
        table.split('.').map(|p| quote_if_needed(p, kind)).collect::<Vec<_>>().join(".")
    } else if table.contains(['"', '`', '[']) {
        table.to_string()
    } else {
        quote_if_needed(table, kind)
    };
    let cols: Vec<String> = result.columns.iter().map(|c| quote_if_needed(&c.name, kind)).collect();
    let mut out = String::new();
    for row in &result.rows {
        let vals: Vec<String> = row.iter().map(|v| sql_literal(v, kind)).collect();
        out.push_str(&format!("INSERT INTO {target} ({}) VALUES ({});\n", cols.join(", "), vals.join(", ")));
    }
    out
}

/// Serialise a result set. CSV uses CRLF line endings as RFC 4180 specifies; NULL is
/// an empty unquoted field and the empty string is `""`.
pub fn export(result: &ResultSet, format: &ExportFormat, kind: DatabaseKind) -> String {
    match format {
        ExportFormat::Csv => csv(result),
        ExportFormat::Tsv => tsv(result),
        ExportFormat::Json => json(result),
        ExportFormat::Markdown => markdown(result),
        ExportFormat::SqlInserts { table } => sql_inserts(result, table, kind),
    }
}

/// Incremental encoder for exports too large to hold in memory: feed it pages of rows and write what it
/// returns. The concatenation of `begin`, every `rows` call and `finish` equals [`export`] over all rows
/// (for CSV, TSV, JSON and SQL INSERTs; Markdown needs every row to align its columns, so it is not
/// streamable and `begin` returns `None` for it).
pub struct ExportEncoder {
    format: ExportFormat,
    kind: DatabaseKind,
    columns: Vec<theta_core::Column>,
    wrote_row: bool,
}

impl ExportEncoder {
    pub fn new(format: ExportFormat, kind: DatabaseKind, columns: Vec<theta_core::Column>) -> Option<Self> {
        (format != ExportFormat::Markdown).then_some(Self { format, kind, columns, wrote_row: false })
    }

    fn chunk(&self, rows: &[Vec<Value>]) -> ResultSet {
        ResultSet { columns: self.columns.clone(), rows: rows.to_vec(), truncated: false }
    }

    /// Text that precedes the rows: the header line, or the opening bracket of a JSON array.
    pub fn begin(&mut self) -> String {
        match self.format {
            ExportFormat::Csv | ExportFormat::Tsv => export(&self.chunk(&[]), &self.format, self.kind),
            ExportFormat::Json => "[\n".into(),
            _ => String::new(),
        }
    }

    /// Text for one page of rows.
    pub fn rows(&mut self, rows: &[Vec<Value>]) -> String {
        if rows.is_empty() {
            return String::new();
        }
        let text = export(&self.chunk(rows), &self.format, self.kind);
        match self.format {
            ExportFormat::Csv | ExportFormat::Tsv => {
                let header = export(&self.chunk(&[]), &self.format, self.kind);
                text[header.len()..].to_string()
            }
            ExportFormat::Json => {
                let body = text.strip_prefix("[\n").and_then(|t| t.strip_suffix("\n]\n")).unwrap_or(&text);
                let sep = if self.wrote_row { ",\n" } else { "" };
                self.wrote_row = true;
                format!("{sep}{body}")
            }
            _ => text,
        }
    }

    /// Text that closes the output.
    pub fn finish(&mut self) -> String {
        match self.format {
            ExportFormat::Json if self.wrote_row => "\n]\n".into(),
            ExportFormat::Json => "]\n".into(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;
    use theta_core::{Column, TypeKind};

    fn rs(cols: &[&str], rows: Vec<Vec<Value>>) -> ResultSet {
        ResultSet {
            columns: cols
                .iter()
                .map(|n| Column {
                    name: n.to_string(),
                    type_name: "text".into(),
                    kind: TypeKind::Text,
                    origin: None,
                })
                .collect(),
            rows,
            truncated: false,
        }
    }

    #[test]
    fn csv_quoting() {
        let r = rs(
            &["a", "b,c"],
            vec![
                vec![Value::Text("x\"y".into()), Value::Text("line1\nline2".into())],
                vec![Value::Null, Value::Text(String::new())],
            ],
        );
        let out = export(&r, &ExportFormat::Csv, Postgres);
        assert_eq!(out, "a,\"b,c\"\r\n\"x\"\"y\",\"line1\nline2\"\r\n,\"\"\r\n");
    }

    #[test]
    fn csv_values() {
        let r = rs(
            &["n", "f", "t", "b"],
            vec![vec![Value::Int(1), Value::Float(2.5), Value::Bool(true), Value::Bytes(vec![255, 0])]],
        );
        assert_eq!(export(&r, &ExportFormat::Csv, Postgres), "n,f,t,b\r\n1,2.5,true,\\xff00\r\n");
    }

    #[test]
    fn csv_bytes_are_not_truncated() {
        let r = rs(&["b"], vec![vec![Value::Bytes(vec![1; 100])]]);
        let out = export(&r, &ExportFormat::Csv, Postgres);
        assert!(out.contains(&"01".repeat(100)));
    }

    #[test]
    fn tsv_escapes_control_characters() {
        let r = rs(&["a"], vec![vec![Value::Text("a\tb\nc\\".into())]]);
        assert_eq!(export(&r, &ExportFormat::Tsv, Postgres), "a\na\\tb\\nc\\\\\n");
    }

    #[test]
    fn json_export_types() {
        let r = rs(
            &["i", "d", "s", "j", "n", "arr"],
            vec![vec![
                Value::Int(1),
                Value::Decimal("12345678901234567890.123".into()),
                Value::Text("q\"\n".into()),
                Value::Json("{\"a\":[1,2]}".into()),
                Value::Null,
                Value::Array(vec![Value::Int(1), Value::Null]),
            ]],
        );
        let out = export(&r, &ExportFormat::Json, Postgres);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed[0]["s"], "q\"\n");
        assert_eq!(parsed[0]["j"]["a"][1], 2);
        assert!(parsed[0]["n"].is_null());
        // Exact decimals survive as written.
        assert!(out.contains("12345678901234567890.123"));
    }

    #[test]
    fn json_empty_and_invalid_json_value() {
        assert_eq!(export(&rs(&["a"], vec![]), &ExportFormat::Json, Postgres), "[]\n");
        let r = rs(&["j"], vec![vec![Value::Json("{broken".into())]]);
        let out = export(&r, &ExportFormat::Json, Postgres);
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
        let r = rs(&["d"], vec![vec![Value::Decimal("+1".into())], vec![Value::Float(f64::NAN)]]);
        assert!(
            serde_json::from_str::<serde_json::Value>(&export(&r, &ExportFormat::Json, Postgres)).is_ok()
        );
    }

    #[test]
    fn markdown_table_alignment_and_escaping() {
        let r = rs(
            &["name", "qty"],
            vec![vec![Value::Text("a|b".into()), Value::Int(5)], vec![Value::Null, Value::Int(1000)]],
        );
        let out = export(&r, &ExportFormat::Markdown, Postgres);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "| name | qty  |");
        assert_eq!(lines[1], "| ---- | ---: |");
        assert_eq!(lines[2], "| a\\|b |    5 |");
        assert_eq!(lines[3], "| NULL | 1000 |");
    }

    #[test]
    fn sql_inserts_per_dialect() {
        let r = rs(&["id", "Name"], vec![vec![Value::Int(1), Value::Text("O'Neil".into())]]);
        let pg = export(&r, &ExportFormat::SqlInserts { table: "public.t".into() }, Postgres);
        assert_eq!(pg, "INSERT INTO public.t (id, \"Name\") VALUES (1, 'O''Neil');\n");
        let my = export(&r, &ExportFormat::SqlInserts { table: "order".into() }, MySql);
        assert_eq!(my, "INSERT INTO `order` (id, Name) VALUES (1, 'O''Neil');\n");
    }

    #[test]
    fn streamed_export_equals_one_shot_export() {
        let rows: Vec<Vec<Value>> = (0..7)
            .map(|i| vec![Value::Int(i), if i % 3 == 0 { Value::Null } else { Value::Text(format!("a\"b,{i}\n")) }])
            .collect();
        let all = rs(&["id", "t"], rows.clone());
        for fmt in [
            ExportFormat::Csv,
            ExportFormat::Tsv,
            ExportFormat::Json,
            ExportFormat::SqlInserts { table: "t".into() },
        ] {
            let mut enc = ExportEncoder::new(fmt.clone(), Postgres, all.columns.clone()).unwrap();
            let mut out = enc.begin();
            for page in rows.chunks(3) {
                out.push_str(&enc.rows(page));
            }
            out.push_str(&enc.finish());
            assert_eq!(out, export(&all, &fmt, Postgres), "{fmt:?}");
        }
        // No rows at all still yields a valid document.
        let mut enc = ExportEncoder::new(ExportFormat::Json, Postgres, all.columns.clone()).unwrap();
        assert_eq!(format!("{}{}", enc.begin(), enc.finish()), "[\n]\n");
        assert!(ExportEncoder::new(ExportFormat::Markdown, Postgres, vec![]).is_none());
    }

    #[test]
    fn array_cells_are_pg_array_literals() {
        let r = rs(
            &["a"],
            vec![vec![Value::Array(vec![Value::Text("a b".into()), Value::Text("c".into()), Value::Null])]],
        );
        assert_eq!(export(&r, &ExportFormat::Tsv, Postgres), "a\n{\"a b\",c,NULL}\n");
    }
}
