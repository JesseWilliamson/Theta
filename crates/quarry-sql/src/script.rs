//! Script handling: statement splitting, "statement under the cursor" and
//! statement classification.

use crate::lexer::{Token, TokenKind, tokenize};
use quarry_core::DatabaseKind;
use std::ops::Range;

/// A statement inside a script. `range` is a byte range, trimmed of surrounding
/// whitespace and excluding the terminating `;`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSpan {
    pub range: Range<usize>,
}

/// Lexer-level slice of a script between two top-level semicolons.
#[derive(Debug, Clone)]
pub(crate) struct Segment {
    /// First byte after the previous `;` (0 for the first segment).
    pub start: usize,
    /// Byte position of the terminating `;`, or `sql.len()`.
    pub end: usize,
    pub has_semi: bool,
    /// Indexes into the token vector, excluding the `;` token itself.
    pub tokens: Range<usize>,
}

fn next_sig_word<'a>(sql: &'a str, toks: &[Token], from: usize) -> Option<&'a str> {
    toks[from..].iter().find(|t| !t.is_trivia()).filter(|t| t.kind == TokenKind::Word).map(|t| t.text(sql))
}

/// Does this statement define a stored routine / trigger whose body may contain `;`?
fn is_routine_header(sql: &str, toks: &[Token], first_sig: usize) -> Option<bool> {
    let mut seen = 0;
    for (n, t) in toks[first_sig..].iter().filter(|t| !t.is_trivia()).enumerate() {
        if n == 0 {
            if !t.is_kw(sql, "CREATE") {
                return Some(false);
            }
            continue;
        }
        seen += 1;
        if t.kind == TokenKind::Word {
            let w = t.text(sql).to_ascii_uppercase();
            match w.as_str() {
                "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT" => return Some(true),
                "TABLE" | "VIEW" | "INDEX" | "SCHEMA" | "DATABASE" | "SEQUENCE" | "TYPE" | "EXTENSION"
                | "DOMAIN" | "ROLE" | "USER" | "VIRTUAL" => return Some(false),
                _ => {}
            }
        }
        if seen > 14 {
            return Some(false);
        }
    }
    None
}

pub(crate) fn segments(sql: &str, toks: &[Token], kind: DatabaseKind) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut seg_start_tok = 0;
    let mut seg_start_byte = 0;
    let mut first_sig: Option<usize> = None;
    let mut routine: Option<bool> = None;
    let mut block_depth = 0usize;
    let mut case_depth = 0usize;
    let mut prev_was_end = false;

    for (i, t) in toks.iter().enumerate() {
        if t.is_trivia() {
            continue;
        }
        if first_sig.is_none() {
            first_sig = Some(i);
        }
        if routine.is_none() {
            routine = is_routine_header(sql, toks, first_sig.unwrap_or(i));
        }
        if t.kind == TokenKind::Word && routine == Some(true) {
            if t.is_kw(sql, "BEGIN") {
                // Postgres 14+ `BEGIN ATOMIC ... END` is the only non-dollar-quoted body.
                let atomic =
                    next_sig_word(sql, toks, i + 1).is_some_and(|w| w.eq_ignore_ascii_case("ATOMIC"));
                if kind != DatabaseKind::Postgres || atomic {
                    block_depth += 1;
                }
            } else if t.is_kw(sql, "CASE") && !prev_was_end {
                // (`END CASE` was already accounted for when we saw the END.)
                case_depth += 1;
            } else if t.is_kw(sql, "END") {
                match next_sig_word(sql, toks, i + 1).map(|w| w.to_ascii_uppercase()).as_deref() {
                    Some("CASE") => case_depth = case_depth.saturating_sub(1),
                    Some("IF" | "LOOP" | "WHILE" | "REPEAT") => {}
                    _ if case_depth > 0 => case_depth -= 1,
                    _ => block_depth = block_depth.saturating_sub(1),
                }
            }
        }
        prev_was_end = t.is_kw(sql, "END");
        if t.kind == TokenKind::Punct && t.is_punct(sql, ";") && block_depth == 0 && case_depth == 0 {
            out.push(Segment {
                start: seg_start_byte,
                end: t.start,
                has_semi: true,
                tokens: seg_start_tok..i,
            });
            seg_start_tok = i + 1;
            seg_start_byte = t.end;
            first_sig = None;
            routine = None;
        }
    }
    out.push(Segment {
        start: seg_start_byte,
        end: sql.len(),
        has_semi: false,
        tokens: seg_start_tok..toks.len(),
    });
    out
}

/// Trimmed byte range of the non-whitespace content, or `None` when the segment
/// holds only whitespace/comments.
pub(crate) fn segment_content(toks: &[Token], seg: &Segment) -> Option<Range<usize>> {
    let ts = &toks[seg.tokens.clone()];
    if ts.iter().all(|t| t.is_trivia()) {
        return None;
    }
    let first = ts.iter().find(|t| t.kind != TokenKind::Whitespace)?;
    let last = ts.iter().rev().find(|t| t.kind != TokenKind::Whitespace)?;
    Some(first.start..last.end)
}

/// Split a script into statements. Comment-only and empty statements are dropped.
pub fn split_statements(sql: &str, kind: DatabaseKind) -> Vec<StatementSpan> {
    let toks = tokenize(sql, kind);
    segments(sql, &toks, kind)
        .iter()
        .filter_map(|s| segment_content(&toks, s))
        .map(|range| StatementSpan { range })
        .collect()
}

/// The statement the cursor is in or touching. A cursor right after the `;`, or in
/// trailing whitespace on a statement's last line, still selects that statement —
/// that is what Ctrl+Enter should run.
pub fn statement_at(sql: &str, offset: usize, kind: DatabaseKind) -> Option<StatementSpan> {
    let mut offset = offset.min(sql.len());
    while !sql.is_char_boundary(offset) {
        offset -= 1;
    }
    let toks = tokenize(sql, kind);
    // (content range, end including the `;`)
    let stmts: Vec<(Range<usize>, usize)> = segments(sql, &toks, kind)
        .iter()
        .filter_map(|s| {
            segment_content(&toks, s).map(|r| (r.clone(), if s.has_semi { s.end + 1 } else { r.end }))
        })
        .collect();
    if stmts.is_empty() {
        return None;
    }
    // Inside a statement, or touching its terminator. Prefer the earlier statement
    // when both touch, so `select 1;|select 2` runs the first one.
    for (r, full_end) in &stmts {
        if offset >= r.start && offset <= *full_end {
            return Some(StatementSpan { range: r.clone() });
        }
    }
    let same_line = |a: usize, b: usize| !sql[a.min(b)..a.max(b)].contains('\n');
    // In whitespace between statements. A statement starting later on this line
    // wins (cursor in its leading indentation); otherwise the one above.
    let prev = stmts.iter().rev().find(|(_, e)| *e < offset);
    let next = stmts.iter().find(|(r, _)| r.start > offset);
    if let Some((r, _)) = next
        && same_line(offset, r.start)
    {
        return Some(StatementSpan { range: r.clone() });
    }
    if let Some((r, _)) = prev {
        return Some(StatementSpan { range: r.clone() });
    }
    next.map(|(r, _)| StatementSpan { range: r.clone() })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementClass {
    /// Returns rows and cannot modify anything.
    Query,
    /// INSERT/UPDATE/DELETE/MERGE/COPY/TRUNCATE, including data-modifying CTEs.
    Write,
    /// CREATE/ALTER/DROP and friends.
    Ddl,
    Transaction,
    /// SET/USE/PRAGMA assignments and other session state.
    Session,
    Other,
}

/// Classify one statement. Leading comments are ignored.
pub fn classify(statement: &str, kind: DatabaseKind) -> StatementClass {
    use StatementClass::*;
    let toks = tokenize(statement, kind);
    let sig: Vec<&Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
    let Some(first) = sig.first() else { return Other };
    if first.is_punct(statement, "(") {
        return Query;
    }
    if first.kind != TokenKind::Word {
        return Other;
    }
    let up = |t: &Token| t.text(statement).to_ascii_uppercase();
    match up(first).as_str() {
        "SELECT" => {
            if kind == DatabaseKind::Postgres && has_top_level_into(statement, &sig) {
                Ddl
            } else {
                Query
            }
        }
        "VALUES" | "SHOW" | "DESCRIBE" | "DESC" | "HELP" => Query,
        "TABLE" if kind == DatabaseKind::Postgres => Query,
        "EXPLAIN" => explain_class(statement, kind, &sig),
        "WITH" => with_class(statement, &sig),
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "COPY" | "TRUNCATE" | "UPSERT" | "LOAD" => {
            Write
        }
        "CREATE" | "ALTER" | "DROP" | "RENAME" | "COMMENT" | "GRANT" | "REVOKE" | "REINDEX" | "VACUUM"
        | "ANALYZE" | "ANALYSE" | "CLUSTER" | "REFRESH" | "ATTACH" | "DETACH" | "OPTIMIZE" | "REPAIR"
        | "SECURITY" | "IMPORT" => Ddl,
        "BEGIN" | "START" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" | "END" | "ABORT" => Transaction,
        "SET" | "RESET" | "USE" | "DISCARD" | "LISTEN" | "UNLISTEN" | "NOTIFY" | "LOCK" | "UNLOCK"
        | "DECLARE" | "FETCH" | "MOVE" | "CLOSE" => Session,
        "PRAGMA" => {
            if sig.iter().any(|t| t.text(statement) == "=" || t.is_punct(statement, "(")) {
                Session
            } else {
                Query
            }
        }
        _ => Other,
    }
}

fn has_top_level_into(sql: &str, sig: &[&Token]) -> bool {
    let mut depth = 0i32;
    for t in sig {
        if t.is_punct(sql, "(") {
            depth += 1;
        } else if t.is_punct(sql, ")") {
            depth -= 1;
        } else if depth == 0 && t.is_kw(sql, "INTO") {
            return true;
        }
    }
    false
}

fn explain_class(sql: &str, kind: DatabaseKind, sig: &[&Token]) -> StatementClass {
    let mut analyze = false;
    let mut inner: Option<StatementClass> = None;
    let mut i = 1;
    let mut depth = 0;
    while i < sig.len() {
        let t = sig[i];
        if t.is_punct(sql, "(") {
            depth += 1;
        } else if t.is_punct(sql, ")") {
            depth -= 1;
        } else if t.kind == TokenKind::Word {
            let w = t.text(sql).to_ascii_uppercase();
            if depth == 0
                && !matches!(
                    w.as_str(),
                    "ANALYZE"
                        | "ANALYSE"
                        | "VERBOSE"
                        | "QUERY"
                        | "PLAN"
                        | "FORMAT"
                        | "EXTENDED"
                        | "PARTITIONS"
                )
            {
                let rest: Vec<String> = sig[i..].iter().map(|t| t.text(sql).to_string()).collect();
                inner = Some(classify(&rest.join(" "), kind));
                break;
            }
            if matches!(w.as_str(), "ANALYZE" | "ANALYSE") {
                analyze = true;
            }
        }
        i += 1;
    }
    match inner {
        Some(StatementClass::Write | StatementClass::Ddl) if analyze => StatementClass::Write,
        _ => StatementClass::Query,
    }
}

fn with_class(sql: &str, sig: &[&Token]) -> StatementClass {
    // A DML keyword that opens a CTE body or is the main statement follows `(`/`)`.
    // `FOR UPDATE`, `ON UPDATE`, `DO UPDATE` follow a word, so they do not match.
    let mut main_write = false;
    for (i, t) in sig.iter().enumerate() {
        if t.kind == TokenKind::Word
            && ["INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE"].iter().any(|k| t.is_kw(sql, k))
            && (i == 0 || sig[i - 1].is_punct(sql, "(") || sig[i - 1].is_punct(sql, ")"))
        {
            main_write = true;
        }
    }
    if main_write { StatementClass::Write } else { StatementClass::Query }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    fn split(sql: &str, k: DatabaseKind) -> Vec<&str> {
        split_statements(sql, k).iter().map(|s| &sql[s.range.clone()]).collect()
    }

    #[test]
    fn basic_split_drops_empty() {
        assert_eq!(split("select 1;; ;\n select 2 ;  ", Postgres), vec!["select 1", "select 2"]);
        assert!(split("", Postgres).is_empty());
        assert!(split("  ;  ; -- nothing\n", Postgres).is_empty());
    }

    #[test]
    fn semicolons_in_strings_and_comments() {
        let sql = "select 'a;b'; select \"x;y\" from t; -- c; d\nselect /* ; */ 3";
        assert_eq!(
            split(sql, Postgres),
            vec!["select 'a;b'", "select \"x;y\" from t", "-- c; d\nselect /* ; */ 3"]
        );
    }

    #[test]
    fn leading_comment_stays_with_statement() {
        let got = split("-- first\nselect 1;\n/* two */ select 2;", Postgres);
        assert_eq!(got, vec!["-- first\nselect 1", "/* two */ select 2"]);
    }

    #[test]
    fn postgres_dollar_quoted_function_body() {
        let sql = "create function f() returns int as $$ begin select 1; return 2; end; $$ language plpgsql; select 3;";
        let got = split(sql, Postgres);
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("plpgsql"));
        let tagged = "do $body$ begin perform 1; end $body$; select 1";
        assert_eq!(split(tagged, Postgres).len(), 2);
    }

    #[test]
    fn mysql_backslash_and_hash_comment() {
        let sql = "select 'it\\'s;ok'; # note; here\nselect 2";
        assert_eq!(split(sql, MySql), vec!["select 'it\\'s;ok'", "# note; here\nselect 2"]);
        // In Postgres `\'` ends the string at the backslash-quote... the quote closes.
        assert_eq!(split("select 'a\\'; select 2", Postgres).len(), 2);
    }

    #[test]
    fn postgres_escape_string_and_nested_comment() {
        let sql = "select E'a\\';b'; /* a /* ; */ ; */ select 2";
        assert_eq!(split(sql, Postgres), vec!["select E'a\\';b'", "/* a /* ; */ ; */ select 2"]);
    }

    #[test]
    fn mysql_procedure_body() {
        let sql = "CREATE PROCEDURE p() BEGIN SELECT 1; IF x THEN SELECT 2; END IF; CASE WHEN a THEN SELECT 3; END CASE; END; SELECT 9;";
        let got = split(sql, MySql);
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("END"));
        assert_eq!(got[1], "SELECT 9");
    }

    #[test]
    fn mysql_function_with_case_expression() {
        let sql = "CREATE FUNCTION f(x INT) RETURNS INT BEGIN RETURN CASE WHEN x > 1 THEN 1 ELSE 0 END; END; SELECT 1";
        let got = split(sql, MySql);
        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("END"));
    }

    #[test]
    fn sqlite_trigger_body() {
        let sql = "CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET x = 1; INSERT INTO c VALUES (1); END; SELECT 1;";
        let got = split(sql, Sqlite);
        assert_eq!(got.len(), 2);
        assert!(got[0].starts_with("CREATE TRIGGER") && got[0].ends_with("END"));
    }

    #[test]
    fn begin_transaction_is_not_a_block() {
        assert_eq!(split("BEGIN; select 1; COMMIT;", Sqlite), vec!["BEGIN", "select 1", "COMMIT"]);
        assert_eq!(split("BEGIN; select 1; COMMIT;", MySql).len(), 3);
    }

    #[test]
    fn postgres_begin_atomic() {
        let sql =
            "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END; SELECT 3";
        assert_eq!(split(sql, Postgres).len(), 2);
    }

    #[test]
    fn unterminated_input_is_tolerated() {
        assert_eq!(split("select 'abc; select 2", Postgres).len(), 1);
        assert_eq!(split("select 1; /* open", Postgres).len(), 1);
        assert_eq!(split("create procedure p() begin select 1;", MySql).len(), 1);
    }

    #[test]
    fn crlf_and_unicode() {
        let sql = "select 'é';\r\nselect 2\r\n";
        let got = split(sql, Postgres);
        assert_eq!(got, vec!["select 'é'", "select 2"]);
    }

    fn at(sql: &str, marker: usize) -> Option<String> {
        statement_at(sql, marker, Postgres).map(|s| sql[s.range].to_string())
    }

    #[test]
    fn statement_at_inside_and_touching() {
        let sql = "select 1;\nselect 2;\n\nselect 3";
        assert_eq!(at(sql, 3).as_deref(), Some("select 1"));
        assert_eq!(at(sql, 9).as_deref(), Some("select 1")); // right after ';'
        assert_eq!(at(sql, 10).as_deref(), Some("select 2"));
        assert_eq!(at(sql, sql.len()).as_deref(), Some("select 3"));
    }

    #[test]
    fn statement_at_trailing_whitespace_and_blank_lines() {
        let sql = "select 1   \n\n\n;select 2";
        assert_eq!(at(sql, 11).as_deref(), Some("select 1")); // trailing spaces
        assert_eq!(at(sql, 13).as_deref(), Some("select 1")); // blank line below it
        let indented = "select 1;\n   select 2";
        assert_eq!(at(indented, 11).as_deref(), Some("select 2")); // leading indentation
    }

    #[test]
    fn statement_at_two_on_one_line() {
        let sql = "select 1; select 2;";
        assert_eq!(at(sql, 9).as_deref(), Some("select 1"));
        assert_eq!(at(sql, 10).as_deref(), Some("select 2"));
        assert_eq!(at(sql, 12).as_deref(), Some("select 2"));
    }

    #[test]
    fn statement_at_empty_script() {
        assert_eq!(at("  \n ", 1), None);
        assert_eq!(at("", 0), None);
    }

    #[test]
    fn statement_at_non_boundary_offset_does_not_panic() {
        let sql = "select 'é'";
        assert!(statement_at(sql, 9, Postgres).is_some());
        assert!(statement_at(sql, 999, Postgres).is_some());
    }

    #[test]
    fn classify_queries() {
        for s in [
            "select 1",
            "  -- c\n SELECT * from t",
            "(select 1) union (select 2)",
            "show tables",
            "values (1)",
            "explain select 1",
            "with a as (select 1) select * from a",
            "select * from t for update",
        ] {
            assert_eq!(classify(s, Postgres), StatementClass::Query, "{s}");
        }
    }

    #[test]
    fn classify_writes() {
        for s in [
            "insert into t values (1)",
            "UPDATE t SET a=1",
            "delete from t",
            "merge into t using s on true when matched then delete",
            "copy t from stdin",
            "truncate t",
            "with d as (delete from t returning *) select * from d",
            "with a as (select 1) insert into t select * from a",
            "with a as (select 1) update t set x = 1",
            "explain analyze delete from t",
        ] {
            assert_eq!(classify(s, Postgres), StatementClass::Write, "{s}");
        }
        assert_eq!(classify("replace into t values (1)", MySql), StatementClass::Write);
    }

    #[test]
    fn explain_without_analyze_is_a_query() {
        assert_eq!(classify("explain delete from t", Postgres), StatementClass::Query);
        assert_eq!(
            classify("explain (analyze, buffers) update t set a = 1", Postgres),
            StatementClass::Write
        );
        assert_eq!(classify("EXPLAIN QUERY PLAN select 1", Sqlite), StatementClass::Query);
    }

    #[test]
    fn classify_ddl_tx_session() {
        assert_eq!(classify("create table t(a int)", Postgres), StatementClass::Ddl);
        assert_eq!(classify("DROP INDEX i", MySql), StatementClass::Ddl);
        assert_eq!(classify("alter table t add column a int", Postgres), StatementClass::Ddl);
        assert_eq!(classify("select * into newt from t", Postgres), StatementClass::Ddl);
        assert_eq!(classify("begin", Postgres), StatementClass::Transaction);
        assert_eq!(classify("ROLLBACK TO SAVEPOINT a", Postgres), StatementClass::Transaction);
        assert_eq!(classify("set search_path = a", Postgres), StatementClass::Session);
        assert_eq!(classify("use shop", MySql), StatementClass::Session);
        assert_eq!(classify("pragma foreign_keys = on", Sqlite), StatementClass::Session);
        assert_eq!(classify("pragma table_info(t)", Sqlite), StatementClass::Session);
        assert_eq!(classify("pragma user_version", Sqlite), StatementClass::Query);
        assert_eq!(classify("call p()", MySql), StatementClass::Other);
        assert_eq!(classify("", Postgres), StatementClass::Other);
    }

    #[test]
    fn classify_on_conflict_do_update_is_write_not_confused() {
        assert_eq!(
            classify("insert into t values (1) on conflict do update set a = 1", Postgres),
            StatementClass::Write
        );
        assert_eq!(classify("select 1 from t for update", Postgres), StatementClass::Query);
    }
}
