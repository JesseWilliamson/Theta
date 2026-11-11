//! A streaming, RFC 4180 style delimited-text reader.
//!
//! Design constraints, in order of importance:
//!
//! * **Bounded memory.** Input is pulled one decoded chunk at a time ([`TextChunks`]); the only thing that
//!   grows is the current record, capped by [`CsvOptions::max_field_bytes`] per field.
//! * **Never panic, never loop on hostile input.** Every byte of progress is consumed from the chunk, and
//!   the only fatal conditions are I/O errors, an oversized field and a quote still open at end of file.
//! * **Lenient where spreadsheets are sloppy.** A quote in the middle of an unquoted field is literal;
//!   text after a closing quote is appended; rows may have any number of fields (the caller decides what
//!   ragged means); CRLF, LF and lone CR all end a record.
//! * **Quotedness survives.** Each [`Field`] remembers whether it was quoted, because `,,` (NULL) and
//!   `,"",` (empty string) must stay distinguishable: that is what Theta's own CSV export writes.
//!
//! Delimiter, quote and escape are single ASCII bytes. That keeps the hot loop byte-oriented while the text
//! stays valid UTF-8 (a split at an ASCII byte never lands inside a character).

use crate::decode::{Encoding, TextChunks};
use std::io::{self, Read};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvOptions {
    pub delimiter: u8,
    /// `None` disables quoting entirely (every byte is data).
    pub quote: Option<u8>,
    /// `None`: a quote inside a quoted field is written twice. `Some(b'\\')`: a backslash escapes the next
    /// byte inside quotes (MySQL `SELECT ... INTO OUTFILE` style), and doubled quotes still work.
    pub escape: Option<u8>,
    pub encoding: Encoding,
    /// A single field larger than this is a hard error rather than an out-of-memory crash.
    pub max_field_bytes: usize,
    /// Records consisting of nothing (a blank line) are skipped.
    pub skip_empty_lines: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: Some(b'"'),
            escape: None,
            encoding: Encoding::Auto,
            max_field_bytes: 256 * 1024 * 1024,
            skip_empty_lines: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Field {
    pub text: String,
    pub quoted: bool,
}

/// One parsed record. Reused between calls so a million-row file does not allocate a million `Vec`s.
#[derive(Debug, Clone, Default)]
pub struct Record {
    fields: Vec<Field>,
    count: usize,
    /// 1-based physical line on which the record starts.
    pub line: u64,
    /// 1-based ordinal among the records returned (the header, when present, is record 1).
    pub index: u64,
}

impl Record {
    pub fn fields(&self) -> &[Field] {
        &self.fields[..self.count]
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn reset(&mut self) {
        self.count = 0;
    }

    fn open_field(&mut self) -> &mut Field {
        if self.count == self.fields.len() {
            self.fields.push(Field::default());
        }
        let f = &mut self.fields[self.count];
        f.text.clear();
        f.quoted = false;
        f
    }

    fn current(&mut self) -> &mut Field {
        &mut self.fields[self.count]
    }

    fn close_field(&mut self) {
        self.count += 1;
    }
}

#[derive(Debug)]
pub enum ParseError {
    Io(io::Error),
    /// A quoted field starts on `line` and the file ends before it closes.
    UnterminatedQuote {
        line: u64,
    },
    FieldTooLarge {
        line: u64,
        limit: usize,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "read error: {e}"),
            ParseError::UnterminatedQuote { line } => {
                write!(f, "a quoted field starting on line {line} is never closed")
            }
            ParseError::FieldTooLarge { line, limit } => {
                write!(f, "a field on line {line} is larger than {limit} bytes")
            }
        }
    }
}

impl std::error::Error for ParseError {}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> Self {
        ParseError::Io(e)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    FieldStart,
    Unquoted,
    Quoted,
    /// Saw a quote inside a quoted field: either the closer or the first half of `""`.
    QuoteSeen,
    /// Saw the escape byte inside a quoted field.
    Escaped,
    /// After a closing quote, before the delimiter or line end.
    AfterQuote,
}

pub struct CsvReader<R: Read> {
    text: TextChunks<R>,
    opts: CsvOptions,
    buf: String,
    pos: usize,
    eof: bool,
    /// The previous record ended with CR; a directly following LF belongs to it.
    skip_lf: bool,
    line: u64,
    next_index: u64,
    failed: bool,
}

impl<R: Read> CsvReader<R> {
    pub fn new(inner: R, opts: CsvOptions) -> Self {
        let text = TextChunks::new(inner, opts.encoding);
        Self {
            text,
            opts,
            buf: String::new(),
            pos: 0,
            eof: false,
            skip_lf: false,
            line: 1,
            next_index: 1,
            failed: false,
        }
    }

    pub fn encoding(&self) -> Encoding {
        self.text.encoding()
    }

    pub fn bytes_consumed(&self) -> u64 {
        self.text.bytes_consumed()
    }

    /// The input contained bytes that are not valid in the chosen encoding (replaced by U+FFFD).
    pub fn had_decode_errors(&self) -> bool {
        self.text.had_errors
    }

    pub fn options(&self) -> &CsvOptions {
        &self.opts
    }

    fn refill(&mut self) -> Result<bool, ParseError> {
        if self.eof {
            return Ok(false);
        }
        if self.text.read_chunk(&mut self.buf)? {
            self.pos = 0;
            Ok(true)
        } else {
            self.eof = true;
            self.buf.clear();
            self.pos = 0;
            Ok(false)
        }
    }

    /// Read the next record into `rec`. `Ok(false)` at the end of the input.
    pub fn read_record(&mut self, rec: &mut Record) -> Result<bool, ParseError> {
        if self.failed {
            return Ok(false);
        }
        let r = self.read_record_inner(rec);
        if r.is_err() {
            // After a fatal error the position is meaningless; stop rather than emit garbage rows.
            self.failed = true;
        }
        r
    }

    fn read_record_inner(&mut self, rec: &mut Record) -> Result<bool, ParseError> {
        let delim = self.opts.delimiter;
        let quote = self.opts.quote;
        let escape = self.opts.escape;
        let limit = self.opts.max_field_bytes;
        loop {
            rec.reset();
            rec.line = self.line;
            let mut state = State::FieldStart;
            let mut quote_line = self.line;
            // Set when the record ended by a line break (as opposed to the end of input).
            let mut ended = false;
            'record: loop {
                if self.pos >= self.buf.len() && !self.refill()? {
                    break 'record;
                }
                let bytes = self.buf.as_bytes();
                // A chunk can begin with the LF of a CRLF whose CR ended the previous chunk's record.
                if self.skip_lf {
                    self.skip_lf = false;
                    if bytes[self.pos] == b'\n' {
                        self.pos += 1;
                        continue;
                    }
                }
                let mut i = self.pos;
                while i < bytes.len() {
                    match state {
                        State::FieldStart | State::Unquoted | State::AfterQuote => {
                            if state == State::FieldStart {
                                rec.open_field();
                                if quote == Some(bytes[i]) {
                                    rec.current().quoted = true;
                                    quote_line = self.line;
                                    state = State::Quoted;
                                    i += 1;
                                    continue;
                                }
                                state = State::Unquoted;
                            }
                            // Scan to the next byte that matters in an unquoted run.
                            let start = i;
                            let rel = bytes[i..].iter().position(|&b| b == delim || b == b'\n' || b == b'\r');
                            let end = rel.map_or(bytes.len(), |r| i + r);
                            if end > start {
                                let f = rec.current();
                                if f.text.len() + (end - start) > limit {
                                    return Err(ParseError::FieldTooLarge { line: self.line, limit });
                                }
                                f.text.push_str(&self.buf[start..end]);
                            }
                            i = end;
                            if i >= bytes.len() {
                                break;
                            }
                            let b = bytes[i];
                            i += 1;
                            if b == delim {
                                rec.close_field();
                                state = State::FieldStart;
                            } else {
                                // CR, LF or CRLF.
                                if b == b'\r' {
                                    if i < bytes.len() {
                                        if bytes[i] == b'\n' {
                                            i += 1;
                                        }
                                    } else {
                                        self.skip_lf = true;
                                    }
                                }
                                rec.close_field();
                                self.pos = i;
                                self.line += 1;
                                ended = true;
                                break 'record;
                            }
                        }
                        State::Quoted => {
                            let q = quote.unwrap_or(0);
                            let start = i;
                            let rel = bytes[i..].iter().position(|&b| b == q || Some(b) == escape);
                            let end = rel.map_or(bytes.len(), |r| i + r);
                            if end > start {
                                self.line += bytes[start..end].iter().filter(|&&b| b == b'\n').count() as u64;
                                let f = rec.current();
                                if f.text.len() + (end - start) > limit {
                                    return Err(ParseError::FieldTooLarge { line: self.line, limit });
                                }
                                f.text.push_str(&self.buf[start..end]);
                            }
                            i = end;
                            if i >= bytes.len() {
                                break;
                            }
                            if Some(bytes[i]) == escape && Some(bytes[i]) != quote {
                                state = State::Escaped;
                            } else {
                                state = State::QuoteSeen;
                            }
                            i += 1;
                        }
                        State::QuoteSeen => {
                            let b = bytes[i];
                            if Some(b) == quote {
                                rec.current().text.push(b as char);
                                state = State::Quoted;
                                i += 1;
                            } else {
                                // The quote closed the field; reprocess this byte as after-quote.
                                state = State::AfterQuote;
                            }
                        }
                        State::Escaped => {
                            // The escaped byte may start a multi-byte character; copy the whole character.
                            let ch = self.buf[i..].chars().next().unwrap_or('\0');
                            if ch == '\n' {
                                self.line += 1;
                            }
                            let f = rec.current();
                            match (ch, escape) {
                                ('n', Some(_)) => f.text.push('\n'),
                                ('t', Some(_)) => f.text.push('\t'),
                                ('r', Some(_)) => f.text.push('\r'),
                                ('0', Some(_)) => f.text.push('\0'),
                                _ => f.text.push(ch),
                            }
                            i += ch.len_utf8();
                            state = State::Quoted;
                        }
                    }
                }
                self.pos = i;
            }
            // End of input inside a record.
            if !ended {
                match state {
                    State::Quoted | State::Escaped => {
                        return Err(ParseError::UnterminatedQuote { line: quote_line });
                    }
                    State::QuoteSeen | State::AfterQuote | State::Unquoted => rec.close_field(),
                    State::FieldStart => {
                        // Nothing was read, or the input ended right after a delimiter, which leaves one
                        // more (empty) field.
                        if rec.count == 0 {
                            return Ok(false);
                        }
                        rec.open_field();
                        rec.close_field();
                    }
                }
            }
            if self.opts.skip_empty_lines
                && rec.count == 1
                && rec.fields[0].text.is_empty()
                && !rec.fields[0].quoted
            {
                if ended {
                    continue;
                }
                return Ok(false);
            }
            if rec.count == 0 {
                if ended {
                    continue;
                }
                return Ok(false);
            }
            rec.index = self.next_index;
            self.next_index += 1;
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_with(input: &[u8], opts: CsvOptions) -> Vec<Vec<(String, bool)>> {
        let mut r = CsvReader::new(input, opts);
        let mut rec = Record::default();
        let mut out = Vec::new();
        while r.read_record(&mut rec).expect("parse") {
            out.push(rec.fields().iter().map(|f| (f.text.clone(), f.quoted)).collect());
        }
        out
    }

    fn rows(input: &str) -> Vec<Vec<String>> {
        parse_with(input.as_bytes(), CsvOptions::default())
            .into_iter()
            .map(|r| r.into_iter().map(|(t, _)| t).collect())
            .collect()
    }

    fn v(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn plain_rows_and_line_endings() {
        let want = v(&[&["a", "b"], &["1", "2"]]);
        assert_eq!(rows("a,b\n1,2\n"), want);
        assert_eq!(rows("a,b\r\n1,2\r\n"), want);
        assert_eq!(rows("a,b\r1,2\r"), want);
        assert_eq!(rows("a,b\n1,2"), want);
        assert_eq!(rows("a,b\r\n1,2"), want);
    }

    #[test]
    fn quoted_newlines_and_doubled_quotes() {
        assert_eq!(
            rows("a,\"x\ny\"\n\"he said \"\"hi\"\"\",2\n"),
            v(&[&["a", "x\ny"], &["he said \"hi\"", "2"]])
        );
        assert_eq!(rows("\"a,b\",c\r\n"), v(&[&["a,b", "c"]]));
        assert_eq!(rows("\"\"\"\"\n"), v(&[&["\""]]));
    }

    #[test]
    fn empty_fields_and_null_vs_empty_string() {
        let r = parse_with(b"a,,\"\",d\n", CsvOptions::default());
        assert_eq!(
            r[0],
            vec![("a".into(), false), (String::new(), false), (String::new(), true), ("d".into(), false)]
        );
        // Trailing delimiter: the last field exists and is empty.
        assert_eq!(rows("a,b,\n"), v(&[&["a", "b", ""]]));
        assert_eq!(rows("a,b,"), v(&[&["a", "b", ""]]));
        assert_eq!(rows(",\n"), v(&[&["", ""]]));
    }

    #[test]
    fn blank_lines_are_skipped_but_can_be_kept() {
        assert_eq!(rows("a\n\n\nb\n"), v(&[&["a"], &["b"]]));
        assert_eq!(rows("\n\na\n"), v(&[&["a"]]));
        let kept = parse_with(b"a\n\nb\n", CsvOptions { skip_empty_lines: false, ..Default::default() });
        assert_eq!(kept.len(), 3);
        // A quoted empty string alone is data, not a blank line.
        assert_eq!(rows("\"\"\n").len(), 1);
    }

    #[test]
    fn ragged_rows_are_reported_as_they_are() {
        assert_eq!(rows("a,b,c\n1\n1,2,3,4\n"), v(&[&["a", "b", "c"], &["1"], &["1", "2", "3", "4"]]));
    }

    #[test]
    fn lenient_quotes() {
        // A quote inside an unquoted field is literal.
        assert_eq!(rows("ab\"cd,e\n"), v(&[&["ab\"cd", "e"]]));
        // Text after a closing quote is appended.
        assert_eq!(rows("\"ab\"cd,e\n"), v(&[&["abcd", "e"]]));
    }

    #[test]
    fn other_delimiters_and_quote_chars() {
        let tsv = CsvOptions { delimiter: b'\t', quote: None, ..Default::default() };
        let r = parse_with(b"a\t\"b\n", tsv);
        assert_eq!(r, vec![vec![("a".into(), false), ("\"b".into(), false)]]);
        let semi = CsvOptions { delimiter: b';', quote: Some(b'\''), ..Default::default() };
        let r = parse_with(b"'a;b';c\n", semi);
        assert_eq!(r[0][0], ("a;b".into(), true));
    }

    #[test]
    fn backslash_escapes_inside_quotes() {
        let o = CsvOptions { escape: Some(b'\\'), ..Default::default() };
        let r = parse_with(b"\"a\\\"b\\nc\\\\\",d\n", o);
        assert_eq!(r[0][0].0, "a\"b\nc\\");
    }

    #[test]
    fn unicode_survives() {
        assert_eq!(rows("héllo,日本語,😀\n"), v(&[&["héllo", "日本語", "😀"]]));
    }

    #[test]
    fn unterminated_quote_is_an_error_with_the_line() {
        let mut r = CsvReader::new(&b"a\n\"oops\nmore"[..], CsvOptions::default());
        let mut rec = Record::default();
        assert!(r.read_record(&mut rec).unwrap());
        match r.read_record(&mut rec) {
            Err(ParseError::UnterminatedQuote { line }) => assert_eq!(line, 2),
            other => panic!("unexpected {other:?}"),
        }
        // Afterwards the reader stays finished.
        assert!(!r.read_record(&mut rec).unwrap());
    }

    #[test]
    fn oversized_field_is_an_error() {
        let o = CsvOptions { max_field_bytes: 8, ..Default::default() };
        let mut r = CsvReader::new(&b"a,0123456789\n"[..], o);
        let mut rec = Record::default();
        assert!(matches!(r.read_record(&mut rec), Err(ParseError::FieldTooLarge { .. })));
    }

    #[test]
    fn huge_field_and_chunk_boundaries() {
        // A 3 MB quoted field with embedded newlines crosses many 64 KB chunks.
        let big = "x\n".repeat(1_500_000);
        let input = format!("id,body\n1,\"{big}\"\n2,tail\n");
        let r = rows(&input);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1][1].len(), big.len());
        assert_eq!(r[2], v(&[&["2", "tail"]])[0]);
    }

    #[test]
    fn every_split_point_gives_the_same_answer() {
        // Feed the same text through a reader that returns 1..=7 bytes at a time: state must carry across
        // reads, including a CR at the end of one read and the LF at the start of the next.
        struct Dribble<'a>(&'a [u8], usize);
        impl Read for Dribble<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.1.min(self.0.len()).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let text = "id,name\r\n1,\"a\r\nb\"\"c\"\r\n2,,\r\n3,é,\"\"\r\n\r\n4,x";
        let want = rows(text);
        for n in 1..=7 {
            let mut r = CsvReader::new(Dribble(text.as_bytes(), n), CsvOptions::default());
            let mut rec = Record::default();
            let mut got = Vec::new();
            while r.read_record(&mut rec).unwrap() {
                got.push(rec.fields().iter().map(|f| f.text.clone()).collect::<Vec<_>>());
            }
            assert_eq!(got, want, "dribble {n}");
        }
    }

    #[test]
    fn record_numbers_and_lines() {
        let mut r = CsvReader::new(&b"a\n\"x\ny\"\nc\n"[..], CsvOptions::default());
        let mut rec = Record::default();
        let mut seen = Vec::new();
        while r.read_record(&mut rec).unwrap() {
            seen.push((rec.index, rec.line));
        }
        assert_eq!(seen, vec![(1, 1), (2, 2), (3, 4)]);
    }

    /// Deterministic pseudo-random bytes (xorshift) so the test needs no rand crate and is reproducible.
    fn noise(seed: &mut u64) -> u8 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed as u8
    }

    #[test]
    fn fuzz_random_bytes_and_truncations_never_panic() {
        let mut seed = 0x9E3779B97F4A7C15u64;
        let alphabet: &[u8] = b"ab,\"\n\r;\t\\ \xC3\xA9\xFF\0";
        for round in 0..400 {
            let len = (noise(&mut seed) as usize) * 4;
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    let b = noise(&mut seed);
                    if round % 2 == 0 { b } else { alphabet[b as usize % alphabet.len()] }
                })
                .collect();
            for opts in [
                CsvOptions::default(),
                CsvOptions { delimiter: b'\t', quote: None, ..Default::default() },
                CsvOptions { escape: Some(b'\\'), encoding: Encoding::Utf16Le, ..Default::default() },
                CsvOptions { encoding: Encoding::Latin1, skip_empty_lines: false, ..Default::default() },
            ] {
                for cut in [data.len(), data.len() / 2, data.len().saturating_sub(1), 0] {
                    let mut r = CsvReader::new(&data[..cut], opts.clone());
                    let mut rec = Record::default();
                    let mut guard = 0usize;
                    while let Ok(true) = r.read_record(&mut rec) {
                        guard += 1;
                        // Every record consumes input, so this bounds the loop.
                        assert!(guard <= cut + 1);
                    }
                }
            }
        }
    }
}
