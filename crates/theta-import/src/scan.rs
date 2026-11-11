//! Look at the start of a file for the dialog: header names, a preview of the first rows and an inferred
//! type per column. Reads at most `sample_rows` records, so it is instant on any file size.

use crate::convert::ConvertOptions;
use crate::decode::Encoding;
use crate::infer::{ColumnType, TypeInferer};
use crate::parser::{CsvOptions, CsvReader, ParseError, Record};
use crate::plan::{column_name_from_header, dedupe};
use std::io::Read;
use theta_core::{Column, ResultSet, TypeKind, Value};

pub const PREVIEW_ROWS: usize = 100;
pub const SAMPLE_ROWS: usize = 2000;

#[derive(Debug, Clone)]
pub struct Scan {
    /// Header cells as written, or `column 1`, `column 2`, ... without a header row.
    pub names: Vec<String>,
    /// Number of fields in the first record.
    pub width: usize,
    pub types: Vec<ColumnType>,
    /// The first [`PREVIEW_ROWS`] data rows as the importer will read them (NULLs resolved).
    pub preview: ResultSet,
    pub encoding: Encoding,
    /// Records in the sample that do not have `width` fields.
    pub ragged: usize,
    pub sampled: usize,
    /// The sample ended inside a quoted field (the file may be truncated, or the quote setting is wrong).
    pub unterminated_quote: bool,
}

impl Scan {
    /// Names for a new table: header cells turned into identifiers, deduplicated.
    pub fn suggested_column_names(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.names.iter().enumerate().map(|(i, n)| column_name_from_header(n, i)).collect();
        dedupe(&mut v);
        v
    }
}

pub fn scan<R: Read>(
    input: R,
    csv: &CsvOptions,
    has_header: bool,
    convert: &ConvertOptions,
    sample_rows: usize,
) -> Result<Scan, ParseError> {
    let mut reader = CsvReader::new(input, csv.clone());
    let mut rec = Record::default();
    let mut names: Vec<String> = Vec::new();
    let mut width = 0;
    let mut first_data: Option<Vec<(String, bool)>> = None;
    if reader.read_record(&mut rec)? {
        width = rec.len();
        if has_header {
            names = rec.fields().iter().map(|f| f.text.trim().to_string()).collect();
        } else {
            first_data = Some(rec.fields().iter().map(|f| (f.text.clone(), f.quoted)).collect());
        }
    }
    if names.is_empty() {
        names = (1..=width).map(|i| format!("column {i}")).collect();
    }
    let mut inf = TypeInferer::new(width);
    let mut rows: Vec<Vec<Value>> = Vec::new();
    let (ragged, sampled) = (std::cell::Cell::new(0usize), std::cell::Cell::new(0usize));
    let mut unterminated_quote = false;
    let handle = |fields: &[(String, bool)], inf: &mut TypeInferer, rows: &mut Vec<Vec<Value>>| {
        sampled.set(sampled.get() + 1);
        if fields.len() != width {
            ragged.set(ragged.get() + 1);
        }
        let is_null = |t: &str, quoted: bool| {
            !quoted
                && ((convert.empty_as_null && t.is_empty())
                    || convert.null_token.as_deref().is_some_and(|n| !n.is_empty() && n == t))
        };
        for (i, (t, q)) in fields.iter().enumerate().take(width) {
            if !is_null(t, *q) && !(t.is_empty() && *q) {
                inf.observe(i, t);
            }
        }
        if rows.len() < PREVIEW_ROWS {
            rows.push(
                (0..width)
                    .map(|i| match fields.get(i) {
                        Some((t, q)) if is_null(t, *q) => Value::Null,
                        Some((t, _)) => Value::Text(t.clone()),
                        None => Value::Null,
                    })
                    .collect(),
            );
        }
    };
    if let Some(f) = first_data.take() {
        handle(&f, &mut inf, &mut rows);
    }
    while sampled.get() < sample_rows {
        match reader.read_record(&mut rec) {
            Ok(true) => {
                let f: Vec<(String, bool)> =
                    rec.fields().iter().map(|f| (f.text.clone(), f.quoted)).collect();
                handle(&f, &mut inf, &mut rows);
            }
            Ok(false) => break,
            Err(ParseError::UnterminatedQuote { .. }) => {
                unterminated_quote = true;
                break;
            }
            Err(e) => return Err(e),
        }
    }
    let types = (0..width).map(|i| inf.infer(i)).collect();
    let columns = names
        .iter()
        .map(|n| Column { name: n.clone(), type_name: "text".into(), kind: TypeKind::Text, origin: None })
        .collect();
    Ok(Scan {
        names,
        width,
        types,
        preview: ResultSet { columns, rows, truncated: false },
        encoding: reader.encoding(),
        ragged: ragged.get(),
        sampled: sampled.get(),
        unterminated_quote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(data: &str, header: bool) -> Scan {
        scan(data.as_bytes(), &CsvOptions::default(), header, &ConvertOptions::default(), SAMPLE_ROWS)
            .unwrap()
    }

    #[test]
    fn header_types_and_nulls() {
        let s = run("id,Name,born,score\n1,Ann,2000-01-02,1.5\n2,,2001-02-03,\n3,\"\",2002-03-04,2\n", true);
        assert_eq!(s.names, vec!["id", "Name", "born", "score"]);
        assert_eq!(s.types[0], ColumnType::Integer);
        assert_eq!(s.types[1], ColumnType::Text { max_len: 3 });
        assert_eq!(s.types[2], ColumnType::Date);
        assert!(matches!(s.types[3], ColumnType::Numeric { .. }));
        assert_eq!(s.preview.rows[1][1], Value::Null);
        // A quoted empty string is data, not NULL.
        assert_eq!(s.preview.rows[2][1], Value::Text(String::new()));
        assert_eq!(s.suggested_column_names(), vec!["id", "name", "born", "score"]);
    }

    #[test]
    fn without_header_the_first_row_is_data() {
        let s = run("1,a\n2,b\n", false);
        assert_eq!(s.names, vec!["column 1", "column 2"]);
        assert_eq!(s.preview.rows.len(), 2);
        assert_eq!(s.types[0], ColumnType::Integer);
        assert_eq!(s.suggested_column_names(), vec!["column_1", "column_2"]);
    }

    #[test]
    fn ragged_and_truncated_samples_are_flagged() {
        let s = run("a,b\n1,2\n3\n4,5,6\n", true);
        assert_eq!(s.ragged, 2);
        let s = run("a,b\n1,\"cut", true);
        assert!(s.unterminated_quote);
        assert_eq!(s.sampled, 0);
    }

    #[test]
    fn preview_is_capped() {
        let mut data = String::from("n\n");
        for i in 0..500 {
            data.push_str(&format!("{i}\n"));
        }
        let s = run(&data, true);
        assert_eq!(s.preview.rows.len(), PREVIEW_ROWS);
        assert_eq!(s.sampled, 500);
    }

    #[test]
    fn empty_file() {
        let s = run("", true);
        assert_eq!((s.width, s.sampled), (0, 0));
    }
}
