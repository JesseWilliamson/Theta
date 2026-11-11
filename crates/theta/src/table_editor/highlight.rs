//! SQL colouring for the DDL preview.
//!
//! The preview shows short, engine-generated statements, so a small scanner is enough: it classifies
//! comments, strings, quoted identifiers, numbers, keywords and type names, and leaves the rest alone.
//! (The editor's own lexer is private to `theta-editor`, and a read-only editor per statement would bring a
//! gutter, scrollbars and a modal keymap along.)

use std::ops::Range;
use theta_core::DatabaseKind;
use theta_sql::keywords::{is_keyword_any, is_type_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Keyword,
    Type,
    String,
    Number,
    Comment,
    Identifier,
}

/// Classified byte ranges of `sql`, in order and not overlapping.
pub fn spans(sql: &str, kind: DatabaseKind) -> Vec<(Range<usize>, Class)> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            out.push((start..i, Class::Comment));
        } else if c == b'\'' || c == b'"' || c == b'`' {
            i += 1;
            while i < b.len() {
                if b[i] == c {
                    // A doubled quote is an escaped quote, not the end.
                    if b.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((start..i, if c == b'\'' { Class::String } else { Class::Identifier }));
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push((start..i, Class::Number));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &sql[start..i];
            if is_type_name(word) {
                out.push((start..i, Class::Type));
            } else if is_keyword_any(word, kind) {
                out.push((start..i, Class::Keyword));
            }
        } else {
            // Step over a whole UTF-8 character.
            i += 1;
            while i < b.len() && !sql.is_char_boundary(i) {
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(sql: &str) -> Vec<(&str, Class)> {
        spans(sql, DatabaseKind::Postgres).into_iter().map(|(r, c)| (&sql[r], c)).collect()
    }

    #[test]
    fn strings_comments_numbers_and_keywords() {
        let got = classes("ALTER TABLE t ADD COLUMN n numeric(12,2) DEFAULT 'it''s' -- done\n;");
        assert!(got.contains(&("ALTER", Class::Keyword)));
        assert!(got.contains(&("numeric", Class::Type)));
        assert!(got.contains(&("12", Class::Number)));
        assert!(got.contains(&("2", Class::Number)));
        assert!(got.contains(&("'it''s'", Class::String)), "{got:?}");
        assert!(got.contains(&("-- done", Class::Comment)));
    }

    #[test]
    fn quoted_identifiers_are_one_token_and_words_inside_strings_are_not_keywords() {
        let got = classes("CREATE TABLE \"select\" (a text DEFAULT 'drop table')");
        assert!(got.contains(&("\"select\"", Class::Identifier)));
        assert!(!got.iter().any(|(t, c)| *t == "drop" && *c == Class::Keyword));
    }

    #[test]
    fn unterminated_input_and_non_ascii_do_not_panic() {
        let _ = spans("COMMENT ON TABLE t IS 'héllo wörld", DatabaseKind::Postgres);
        let _ = spans("SELECT 'a", DatabaseKind::MySql);
        let _ = spans("é–ü", DatabaseKind::Sqlite);
        let ranges: Vec<_> =
            spans("SELECT 1 -- ü", DatabaseKind::Sqlite).into_iter().map(|(r, _)| r).collect();
        assert!(ranges.windows(2).all(|w| w[0].end <= w[1].start));
    }
}
