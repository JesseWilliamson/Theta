//! Clipboard text for a selection.

use crate::format::raw_text;
use quarry_core::Value;

/// One field of a multi-cell TSV. Tabs, newlines and quotes would break the
/// row/column structure when pasted into a spreadsheet, so such fields are
/// quoted the way spreadsheets expect (`"` doubled).
pub fn tsv_field(text: &str) -> String {
    if text.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// `headers` is the header line (already restricted to the copied columns).
/// A single cell without headers copies as its raw value: no quoting, so
/// pasting a multi-line text into an editor gives exactly that text.
pub fn build_tsv(headers: Option<&[String]>, rows: &[Vec<Value>]) -> String {
    if headers.is_none() && rows.len() == 1 && rows[0].len() == 1 {
        return raw_text(&rows[0][0]);
    }
    let mut out = String::new();
    if let Some(h) = headers {
        out.push_str(&h.iter().map(|c| tsv_field(c)).collect::<Vec<_>>().join("\t"));
        out.push('\n');
    }
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut first = true;
        for v in row {
            if !first {
                out.push('\t');
            }
            first = false;
            out.push_str(&tsv_field(&raw_text(v)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn single_cell_is_raw() {
        assert_eq!(build_tsv(None, &[vec![t("a\tb \"q\"\nz")]]), "a\tb \"q\"\nz");
        assert_eq!(build_tsv(None, &[vec![Value::Null]]), "");
        assert_eq!(build_tsv(None, &[vec![Value::Int(7)]]), "7");
    }

    #[test]
    fn multi_cell_tsv() {
        let rows = vec![vec![Value::Int(1), t("x"), Value::Null], vec![Value::Int(2), t("y z"), Value::Bool(true)]];
        assert_eq!(build_tsv(None, &rows), "1\tx\t\n2\ty z\ttrue");
    }

    #[test]
    fn headers_and_quoting() {
        let h = vec!["id".to_string(), "note".to_string()];
        let rows = vec![vec![Value::Int(1), t("line1\nline2")], vec![Value::Int(2), t("say \"hi\"")]];
        assert_eq!(build_tsv(Some(&h), &rows), "id\tnote\n1\t\"line1\nline2\"\n2\t\"say \"\"hi\"\"\"");
    }

    #[test]
    fn single_cell_with_headers_is_tsv() {
        let h = vec!["id".to_string()];
        assert_eq!(build_tsv(Some(&h), &[vec![Value::Int(1)]]), "id\n1");
    }
}
