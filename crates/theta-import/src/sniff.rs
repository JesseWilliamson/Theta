//! Guess the format of a delimited file from its first bytes: encoding, delimiter, quote and whether the
//! first row is a header. Every guess is a default the dialog lets the user override, so the heuristics
//! favour the common case (comma, double quote, header present) when the evidence is thin.

use crate::decode::{Encoding, TextChunks};
use crate::infer::{ColumnType, TypeInferer};
use crate::parser::{CsvOptions, CsvReader, Record};
use crate::scalar::{parse_bool, parse_date, parse_number, parse_timestamp};

/// How much of the file the sniffer looks at.
pub const SAMPLE_BYTES: usize = 128 * 1024;
const SAMPLE_RECORDS: usize = 200;
const DELIMITERS: [u8; 4] = *b",;\t|";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sniffed {
    pub encoding: Encoding,
    pub delimiter: u8,
    pub quote: Option<u8>,
    pub has_header: bool,
}

impl Sniffed {
    pub fn options(&self) -> CsvOptions {
        CsvOptions {
            delimiter: self.delimiter,
            quote: self.quote,
            encoding: self.encoding,
            ..Default::default()
        }
    }
}

/// Decode a sample into text; returns the text and the encoding that was used.
fn decode_sample(bytes: &[u8], enc: Encoding) -> (String, Encoding) {
    let mut chunks = TextChunks::new(bytes, enc);
    let mut text = String::new();
    let mut chunk = String::new();
    while let Ok(true) = chunks.read_chunk(&mut chunk) {
        text.push_str(&chunk);
    }
    (text, chunks.encoding())
}

fn parse_sample(text: &str, opts: &CsvOptions, max: usize) -> Vec<Vec<(String, bool)>> {
    let mut reader = CsvReader::new(text.as_bytes(), CsvOptions { encoding: Encoding::Utf8, ..opts.clone() });
    let mut rec = Record::default();
    let mut rows = Vec::new();
    // A sample cut in the middle of a quoted field ends in an error; the records before it are still good.
    while rows.len() < max && matches!(reader.read_record(&mut rec), Ok(true)) {
        rows.push(rec.fields().iter().map(|f| (f.text.clone(), f.quoted)).collect());
    }
    rows
}

/// Score a delimiter: the share of records with the most common field count, if that count is above one.
fn score(rows: &[Vec<(String, bool)>]) -> (f64, usize) {
    if rows.is_empty() {
        return (0.0, 0);
    }
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for r in rows {
        match counts.iter_mut().find(|(n, _)| *n == r.len()) {
            Some(c) => c.1 += 1,
            None => counts.push((r.len(), 1)),
        }
    }
    let &(n, k) = counts.iter().max_by_key(|(n, k)| (*k, *n)).unwrap_or(&(0, 0));
    if n < 2 {
        return (0.0, n);
    }
    (k as f64 / rows.len() as f64, n)
}

pub fn sniff(bytes: &[u8]) -> Sniffed {
    let head = &bytes[..bytes.len().min(SAMPLE_BYTES)];
    let (text, encoding) = decode_sample(head, Encoding::Auto);
    // The last line of a truncated sample is probably cut short.
    let text = if bytes.len() > head.len() {
        text.rfind('\n').map_or(text.as_str(), |p| &text[..p + 1])
    } else {
        text.as_str()
    };
    let quote = pick_quote(text);
    let mut best: (f64, usize, u8) = (0.0, 0, b',');
    for &d in &DELIMITERS {
        let opts = CsvOptions { delimiter: d, quote, ..Default::default() };
        let rows = parse_sample(text, &opts, SAMPLE_RECORDS);
        let (s, n) = score(&rows);
        // Ties go to the earlier (more common) delimiter, and to the wider table.
        if s > best.0 + 1e-9 || ((s - best.0).abs() < 1e-9 && s > 0.0 && n > best.1 && d != b',') {
            best = (s, n, d);
        }
    }
    let delimiter = best.2;
    let rows = parse_sample(text, &CsvOptions { delimiter, quote, ..Default::default() }, SAMPLE_RECORDS);
    let has_header = looks_like_header(&rows);
    Sniffed { encoding, delimiter, quote, has_header }
}

/// `"` unless the sample clearly uses `'` to enclose fields.
fn pick_quote(text: &str) -> Option<u8> {
    let enclosing = |q: char| {
        text.lines()
            .take(50)
            .map(|l| {
                let starts = l.starts_with(q) as usize;
                let seps = DELIMITERS
                    .iter()
                    .map(|&d| l.matches(&format!("{}{q}", d as char)).count())
                    .sum::<usize>();
                starts + seps
            })
            .sum::<usize>()
    };
    let (dq, sq) = (enclosing('"'), enclosing('\''));
    if sq > 0 && dq == 0 { Some(b'\'') } else { Some(b'"') }
}

fn looks_like_header(rows: &[Vec<(String, bool)>]) -> bool {
    let Some((first, rest)) = rows.split_first() else { return true };
    if rest.is_empty() {
        // One row: a header if it is all words.
        return first.iter().all(|(t, _)| parse_number(t.trim()).is_none());
    }
    let width = first.len();
    let mut inf = TypeInferer::new(width);
    for r in rest.iter().take(100) {
        for (i, (t, _)) in r.iter().enumerate().take(width) {
            if !t.is_empty() {
                inf.observe(i, t);
            }
        }
    }
    let mut typed_columns = 0;
    let mut first_conforms = 0;
    for (i, (cell, _)) in first.iter().enumerate() {
        let ty = inf.infer(i);
        if matches!(ty, ColumnType::Text { .. }) {
            continue;
        }
        typed_columns += 1;
        if conforms(cell.trim(), &ty) {
            first_conforms += 1;
        } else {
            // A word where the data below has numbers or dates is a column title.
            return true;
        }
    }
    // Every typed column's first cell fits its data: the first row is data like any other.
    if typed_columns > 0 && first_conforms == typed_columns {
        return false;
    }
    true
}

fn conforms(v: &str, ty: &ColumnType) -> bool {
    match ty {
        ColumnType::Integer | ColumnType::BigInt => parse_number(v).is_some(),
        ColumnType::Numeric { .. } | ColumnType::Float => parse_number(v).is_some(),
        ColumnType::Boolean => parse_bool(v).is_some(),
        ColumnType::Date => parse_date(v).is_some(),
        ColumnType::Timestamp { .. } => parse_timestamp(v).is_some(),
        ColumnType::Text { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_delimiter() {
        for (d, name) in [(',', "comma"), (';', "semicolon"), ('\t', "tab"), ('|', "pipe")] {
            let text = format!("id{d}name{d}score\n1{d}Ann{d}3.5\n2{d}Bob{d}4.0\n3{d}Cy{d}1.25\n");
            let s = sniff(text.as_bytes());
            assert_eq!(s.delimiter, d as u8, "{name}");
            assert!(s.has_header, "{name}");
        }
    }

    #[test]
    fn commas_inside_quotes_do_not_fool_the_scorer() {
        let text = "name;note\nAnn;\"a, b, c, d\"\nBob;\"x, y, z\"\n";
        assert_eq!(sniff(text.as_bytes()).delimiter, b';');
    }

    #[test]
    fn decimal_comma_with_semicolons() {
        let text = "a;b\n1,5;2,5\n3,5;4,5\n";
        assert_eq!(sniff(text.as_bytes()).delimiter, b';');
    }

    #[test]
    fn single_column_defaults_to_comma() {
        assert_eq!(sniff(b"name\nann\nbob\n").delimiter, b',');
        assert_eq!(sniff(b"").delimiter, b',');
    }

    #[test]
    fn header_heuristic() {
        assert!(sniff(b"id,name\n1,Ann\n2,Bob\n").has_header);
        assert!(!sniff(b"1,Ann\n2,Bob\n3,Cy\n").has_header);
        assert!(sniff(b"when,amount\n2024-01-01,5\n2024-01-02,6\n").has_header);
        assert!(!sniff(b"2024-01-01,5\n2024-01-02,6\n").has_header);
        // All text: assume a header, the common case.
        assert!(sniff(b"first,last\nAnn,Lee\nBob,Ray\n").has_header);
    }

    #[test]
    fn single_quote_enclosure() {
        let s = sniff(b"a,b\n'x, y',1\n'z',2\n");
        assert_eq!(s.quote, Some(b'\''));
        assert_eq!(s.delimiter, b',');
    }

    #[test]
    fn encoding_is_reported() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("a,b\n1,2\n".encode_utf16().flat_map(|u| u.to_le_bytes()));
        let s = sniff(&bytes);
        assert_eq!(s.encoding, Encoding::Utf16Le);
        assert_eq!(s.delimiter, b',');
        assert_eq!(sniff(b"n\ncaf\xE9,1\n").encoding, Encoding::Latin1);
    }

    #[test]
    fn truncated_sample_ignores_the_cut_row() {
        let mut text = String::from("id,name\n");
        for i in 0..40_000 {
            text.push_str(&format!("{i},name{i}\n"));
        }
        let s = sniff(text.as_bytes());
        assert_eq!(s.delimiter, b',');
        assert!(s.has_header);
    }
}
