//! Choosing the statements a run request executes, and the pre-flight checks.

use std::ops::Range;
use theta_core::{DatabaseKind, Error};
use theta_sql::{StatementClass, classify, split_statements, statement_at};

/// One statement to execute: `range` is a byte range in the full document.
#[derive(Debug, Clone, PartialEq)]
pub struct Picked {
    pub range: Range<usize>,
    pub sql: String,
    pub class: StatementClass,
}

fn pick(text: &str, range: Range<usize>, kind: DatabaseKind) -> Picked {
    let sql = text[range.clone()].to_string();
    let class = classify(&sql, kind);
    Picked { range, sql, class }
}

/// The statement under (or, between statements, right after) the cursor.
pub fn at_cursor(text: &str, cursor: usize, kind: DatabaseKind) -> Option<Picked> {
    statement_at(text, cursor, kind).map(|s| pick(text, s.range, kind))
}

/// Every statement of the document in order.
pub fn all(text: &str, kind: DatabaseKind) -> Vec<Picked> {
    split_statements(text, kind).into_iter().map(|s| pick(text, s.range, kind)).collect()
}

/// The statements inside a selection that starts at byte `start` of `text`.
pub fn selection(text: &str, start: usize, selected: &str, kind: DatabaseKind) -> Vec<Picked> {
    split_statements(selected, kind)
        .into_iter()
        .map(|s| {
            let range = start + s.range.start..start + s.range.end;
            let class = classify(&text[range.clone()], kind);
            Picked { sql: text[range.clone()].to_string(), range, class }
        })
        .collect()
}

/// Why a read-only connection refuses this statement.
pub fn read_only_refusal(class: StatementClass, read_only: bool) -> Option<String> {
    if read_only && matches!(class, StatementClass::Write | StatementClass::Ddl) {
        Some("This connection is read-only: statements that modify data or schema are not run.".into())
    } else {
        None
    }
}

/// `UPDATE`/`DELETE` without WHERE: the one case that asks before running.
pub fn dangerous_reason(sql: &str, kind: DatabaseKind, class: StatementClass) -> Option<String> {
    if class != StatementClass::Write {
        return None;
    }
    theta_sql::lint(sql, kind, None)
        .into_iter()
        .find(|d| d.message.contains("without WHERE"))
        .map(|d| d.message)
}

/// First line of a statement for logs, shortened.
pub fn first_line(sql: &str) -> String {
    let line = sql.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > 90 {
        format!("{}…", line.chars().take(90).collect::<String>())
    } else {
        line.to_string()
    }
}

/// Where to put the squiggle for a failed statement. The server's position is a 0-based **byte** offset
/// into the statement that was sent (`theta_core::ErrorPosition`), so it is shifted by the statement's
/// start. Without a position the whole statement is marked. The result is clamped to the document and
/// never splits a character.
pub fn error_range(stmt: &Range<usize>, error: &Error, text: &str) -> Range<usize> {
    let Error::Query { position: Some(p), .. } = error else { return stmt.clone() };
    let stmt_end = stmt.end.min(text.len());
    let mut start = (stmt.start + p.offset).min(stmt_end);
    while start > stmt.start && !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = start;
    // Cover the word at the position, or the one character there when it is punctuation. A position at
    // a line break (or the end of the statement) marks the character before it instead, so the
    // squiggle is never a stray tick after the end of a line.
    let rest = &text[start..stmt_end];
    match rest.chars().next() {
        Some(c) if c.is_alphanumeric() || c == '_' => {
            for (i, c) in rest.char_indices() {
                if c.is_alphanumeric() || c == '_' {
                    end = start + i + c.len_utf8();
                } else {
                    break;
                }
            }
        }
        Some(c) if c != '\n' && c != '\r' => end = start + c.len_utf8(),
        _ => {
            if let Some(prev) = text[stmt.start..start].trim_end_matches(['\n', '\r']).chars().next_back() {
                let before = text[stmt.start..start].trim_end_matches(['\n', '\r']).len();
                start = stmt.start + before - prev.len_utf8();
                end = stmt.start + before;
            } else {
                end = stmt_end.max(start);
            }
        }
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::ErrorPosition;

    const PG: DatabaseKind = DatabaseKind::Postgres;

    #[test]
    fn cursor_picks_statement() {
        let t = "select 1;\nselect 2;\n";
        let p = at_cursor(t, 12, PG).unwrap();
        assert_eq!(p.sql, "select 2");
        assert_eq!(p.class, StatementClass::Query);
    }

    #[test]
    fn all_in_order_with_classes() {
        let t = "select 1; update t set a=1 where b=2; create table x(a int)";
        let v = all(t, PG);
        assert_eq!(v.len(), 3);
        assert_eq!(v[1].class, StatementClass::Write);
        assert_eq!(v[2].class, StatementClass::Ddl);
    }

    #[test]
    fn selection_offsets_into_document() {
        let t = "select 0;\nselect 1; select 2;";
        let start = 10;
        let v = selection(t, start, &t[start..], PG);
        assert_eq!(v.len(), 2);
        assert_eq!(&t[v[1].range.clone()], "select 2");
    }

    #[test]
    fn read_only_refuses_writes_only() {
        assert!(read_only_refusal(StatementClass::Write, true).is_some());
        assert!(read_only_refusal(StatementClass::Ddl, true).is_some());
        assert!(read_only_refusal(StatementClass::Query, true).is_none());
        assert!(read_only_refusal(StatementClass::Write, false).is_none());
    }

    #[test]
    fn dangerous_update_without_where() {
        let sql = "update t set a = 1";
        assert!(dangerous_reason(sql, PG, classify(sql, PG)).is_some());
        let ok = "update t set a = 1 where id = 3";
        assert!(dangerous_reason(ok, PG, classify(ok, PG)).is_none());
        assert!(dangerous_reason("select 1", PG, StatementClass::Query).is_none());
    }

    #[test]
    fn error_position_is_shifted_by_statement_start() {
        let t = "select 1;\nselect nope from t";
        let stmt = 10..t.len();
        assert_eq!(&t[error_range(&stmt, &at(7), t)], "nope");
    }

    fn at(offset: usize) -> Error {
        Error::Query {
            message: "x".into(),
            code: None,
            detail: None,
            hint: None,
            position: Some(ErrorPosition { offset }),
        }
    }

    #[test]
    fn error_after_a_newline_is_on_the_next_line() {
        let t = "select * from aud_t order by id\nnewrow    77";
        let stmt = 0..t.len();
        let off = t.find("newrow").unwrap();
        assert_eq!(&t[error_range(&stmt, &at(off), t)], "newrow");
        // CRLF line ends
        let t = "select *\r\nfrom nope\r\nwhere x";
        let off = t.find("nope").unwrap();
        assert_eq!(&t[error_range(&(0..t.len()), &at(off), t)], "nope");
    }

    #[test]
    fn multibyte_text_before_the_error_does_not_shift_it() {
        let t = "select 'héllo wörld ✓', nope from t";
        let off = t.find("nope").unwrap(); // a byte offset: the string literal has 2-byte and 3-byte chars
        assert_eq!(&t[error_range(&(0..t.len()), &at(off), t)], "nope");
        let t = "select 1;\nselect 'ünï', nope";
        let off = t["select 1;\n".len()..].find("nope").unwrap();
        assert_eq!(&t[error_range(&(10..t.len()), &at(off), t)], "nope");
        // An offset in the middle of a character snaps back to its start.
        let t = "select 'é'";
        let r = error_range(&(0..t.len()), &at(8), t);
        assert!(t.is_char_boundary(r.start) && t.is_char_boundary(r.end));
    }

    #[test]
    fn position_on_a_line_break_or_the_end_marks_the_last_character() {
        let t = "select 1 from\nwhere";
        let r = error_range(&(0..t.len()), &at("select 1 from".len()), t);
        assert_eq!(&t[r], "m");
        let t = "select 1 +";
        let r = error_range(&(0..t.len()), &at(t.len()), t);
        assert_eq!(&t[r], "+");
        let t = "select 1;\nselect nope from t";
        // beyond the statement: clamped
        let r = error_range(&(10..t.len()), &at(500), t);
        assert!(r.end <= t.len() && r.start >= 10);
    }

    #[test]
    fn error_without_position_covers_statement() {
        let t = "select x";
        assert_eq!(error_range(&(0..8), &Error::query("boom"), t), 0..8);
    }

    #[test]
    fn first_line_trims() {
        assert_eq!(first_line("\n  select 1\nfrom t"), "select 1");
    }
}
