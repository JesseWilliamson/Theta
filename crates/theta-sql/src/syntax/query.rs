//! Queries: `WITH`, set operations, `SELECT` and its clauses, `FROM` with joins, `ORDER BY`, `LIMIT`.

use super::node::Kind;
use super::parser::{MAX_DEPTH, Parser};
use super::words;
use crate::lexer::TokenKind;
use theta_core::Family;

impl Parser<'_> {
    /// A full query starting at `WITH` or at a query body.
    pub(super) fn query(&mut self) {
        if self.kw(0, "WITH") {
            self.with_clause();
            let with = self.pop_child();
            self.query_after_with(with);
        } else {
            self.query_after_with(None);
        }
    }

    pub(super) fn query_after_with(&mut self, with: Option<super::node::Node>) {
        if self.depth >= MAX_DEPTH {
            // Nesting this deep is not hand-written SQL. Keep the tokens, skip the structure.
            self.start(Kind::Unknown);
            self.skip_to_close();
            self.finish();
            return;
        }
        self.depth += 1;
        let had_with = with.is_some();
        self.start_adopting(Kind::Query, with);
        if !self.query_body(0) && had_with && self.at_end() {
            // A WITH clause must be followed by the statement it introduces.
            self.error_here("Expected a query after the WITH clause");
        }
        self.query_tail();
        self.finish();
        self.depth -= 1;
    }

    /// `(` Query `)` in expression position.
    pub(super) fn subquery(&mut self) {
        self.start(Kind::Subquery);
        let open = self.next_span();
        self.bump();
        self.query();
        self.expect_close(open);
        self.finish();
    }

    pub(super) fn with_clause(&mut self) {
        self.start(Kind::WithClause);
        self.bump(); // WITH
        self.bump_if_kw("RECURSIVE");
        let mut first = true;
        loop {
            if !self.is_ident(0) || (self.kw_any(0, &["SELECT", "INSERT", "UPDATE", "DELETE"]) && !first) {
                if first && self.at_end() {
                    // `WITH` and nothing after it is not a statement in any dialect.
                    self.error_here("Expected a common table expression after WITH");
                }
                break;
            }
            first = false;
            self.cte();
            if self.punct(0, ",") {
                self.bump();
                if !self.is_ident(0) {
                    self.error_here("Expected a common table expression after the comma");
                    break;
                }
            } else {
                break;
            }
        }
        self.finish();
    }

    fn cte(&mut self) {
        self.start(Kind::Cte);
        self.bump(); // name
        if self.punct(0, "(") {
            self.column_list();
        }
        if self.bump_if_kw("AS") {
            if self.kw(0, "NOT") && self.kw(1, "MATERIALIZED") {
                self.bump();
            }
            self.bump_if_kw("MATERIALIZED");
            if !self.punct(0, "(") {
                // `WITH t AS select ...`: every dialect wraps the CTE body in parentheses.
                if self.at_terminator() || self.is_query_start(0) {
                    self.error_here("Expected ( after AS");
                }
            } else {
                let open = self.next_span();
                self.bump();
                if self.is_query_start(0) || self.punct(0, "(") {
                    self.query();
                } else if self.kw(0, "INSERT") {
                    self.insert(None);
                } else if self.kw(0, "UPDATE") {
                    self.update(None);
                } else if self.kw(0, "DELETE") {
                    self.delete(None);
                } else if self.kw(0, "MERGE") {
                    self.merge(None);
                }
                self.expect_close(open);
            }
        }
        // PostgreSQL `SEARCH ... CYCLE ...` clauses and anything unexpected before the next CTE or the
        // statement proper.
        if !self.punct(0, ",")
            && !self.at_end()
            && !self.kw_any(0, &["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "VALUES", "TABLE", "WITH"])
            && !self.punct(0, "(")
        {
            self.start(Kind::Unknown);
            while let Some(t) = self.peek() {
                if (t.kind == TokenKind::Punct && self.text(&t) == ",")
                    || self.kw_any(0, &["SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "VALUES"])
                {
                    break;
                }
                if self.punct(0, "(") {
                    self.skip_group();
                } else if self.punct(0, ")") {
                    break;
                } else {
                    self.bump();
                }
            }
            self.finish();
        }
        self.finish();
    }

    /// `(a, b, c)`. Tolerant: anything inside is kept as tokens.
    pub(super) fn column_list(&mut self) {
        self.start(Kind::ColumnList);
        let open = self.next_span();
        self.bump();
        // 0: just after `(`, 1: after an item, 2: after a comma
        let mut state = 0;
        while let Some(t) = self.peek() {
            if self.punct(0, ")") {
                if state == 2 && self.rejects_trailing_comma() {
                    self.error(self.prev_span(), "Unexpected comma before ')'", false);
                }
                break;
            }
            if self.punct(0, ",") {
                if state != 1 {
                    // `(,a)` and `(a,,b)`: a name is missing in every dialect.
                    self.error(t.start..t.end, "Unexpected comma: a name is missing", false);
                }
                state = 2;
            } else {
                state = 1;
            }
            if self.punct(0, "(") {
                self.skip_group();
            } else {
                self.bump();
            }
        }
        self.expect_close(open);
        self.finish();
    }

    // ---- set operations

    fn query_body(&mut self, min_prec: u8) -> bool {
        let m = self.mark();
        if !self.query_primary() {
            return false;
        }
        loop {
            let prec = if self.kw_any(0, &["UNION", "EXCEPT", "MINUS"]) {
                1
            } else if self.kw(0, "INTERSECT") {
                2
            } else {
                break;
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            self.bump_if_kw("ALL");
            self.bump_if_kw("DISTINCT");
            if self.kw(0, "BY") && self.kw(1, "NAME") {
                self.bump();
                self.bump();
            }
            if !self.query_body(prec + 1)
                && (self.at_end()
                    || self.punct(0, ")")
                    || self.kw_any(0, &["UNION", "INTERSECT", "EXCEPT", "MINUS"]))
            {
                // A set operator needs a right-hand query in every dialect.
                self.error_here("Expected a query after the set operator");
            }
            let sql = self.sql;
            self.wrap_merging(m, Kind::SetOp, &|n| {
                n.tokens().next().is_some_and(|t| {
                    let intersect = t.text(sql).eq_ignore_ascii_case("INTERSECT");
                    (prec == 2) == intersect
                })
            });
        }
        true
    }

    fn query_primary(&mut self) -> bool {
        if self.punct(0, "(") {
            self.start(Kind::ParenQuery);
            let open = self.next_span();
            self.bump();
            self.query();
            self.expect_close(open);
            self.finish();
            return true;
        }
        if self.kw(0, "SELECT") || (self.kw(0, "FROM") && self.fam == Family::DuckDb) {
            self.select();
            return true;
        }
        if self.kw(0, "VALUES") {
            self.values_body();
            return true;
        }
        if self.kw(0, "TABLE") && self.is_ident(1) {
            self.start(Kind::TableStmt);
            self.bump();
            self.table_name();
            self.finish();
            return true;
        }
        false
    }

    pub(super) fn values_body(&mut self) {
        self.start(Kind::Values);
        self.bump(); // VALUES / VALUE
        let mut rows = 0;
        loop {
            self.bump_if_kw("ROW");
            if self.punct(0, "(") {
                self.row_ctor();
                rows += 1;
            } else {
                break;
            }
            if self.punct(0, ",") {
                self.bump();
            } else {
                break;
            }
        }
        if rows == 0 && (self.at_end() || self.punct(0, ")")) {
            // Every dialect needs at least one parenthesised row after VALUES.
            self.error_here("Expected ( after VALUES");
        }
        self.finish();
    }

    /// `( expr, expr, ... )` as a `RowCtor`; `DEFAULT` is allowed as a value.
    pub(super) fn row_ctor(&mut self) {
        self.start(Kind::RowCtor);
        let open = self.next_span();
        self.bump();
        self.expr_list_until_close();
        self.expect_close(open);
        self.finish();
    }

    /// Comma separated expressions up to (not including) `)`. Unparseable stretches are kept silently.
    pub(super) fn expr_list_until_close(&mut self) {
        // 0: at the start, 1: after an item, 2: after a comma
        let mut state = 0;
        loop {
            if self.at_end() || self.punct(0, ")") {
                if state == 2 && self.rejects_trailing_comma() && !self.at_end() {
                    self.error(self.prev_span(), "Unexpected comma before ')'", false);
                }
                break;
            }
            if self.punct(0, ",") {
                if state != 1 {
                    let sp = self.next_span();
                    self.error(sp, "Unexpected comma: a value is missing", false);
                }
                state = 2;
                self.bump();
                continue;
            }
            if self.expr() {
                state = 1;
                continue;
            }
            state = 1;
            self.start(Kind::Unknown);
            self.bump();
            self.finish();
        }
    }

    // ---- SELECT

    fn select(&mut self) {
        self.start(Kind::Select);
        if self.kw(0, "SELECT") {
            self.bump();
            self.select_modifiers();
            self.select_list();
        }
        self.select_clauses();
        self.finish();
    }

    fn select_modifiers(&mut self) {
        loop {
            if self.kw_any(
                0,
                &[
                    "ALL",
                    "DISTINCTROW",
                    "HIGH_PRIORITY",
                    "SQL_SMALL_RESULT",
                    "SQL_BIG_RESULT",
                    "SQL_BUFFER_RESULT",
                    "SQL_NO_CACHE",
                    "SQL_CACHE",
                    "SQL_CALC_FOUND_ROWS",
                ],
            ) && !self.punct(1, ",")
                && !self.punct(1, "(")
                || (self.kw(0, "STRAIGHT_JOIN") && !self.kw(1, "JOIN") && !self.punct(1, ","))
            {
                self.bump();
            } else if self.kw(0, "DISTINCT") && !self.punct(1, ",") {
                self.bump();
                if self.kw(0, "ON") && self.punct(1, "(") {
                    self.bump();
                    let open = self.next_span();
                    self.bump();
                    self.expr_list_until_close();
                    self.expect_close(open);
                }
            } else if self.kw(0, "TOP")
                && (self.punct(1, "(")
                    || self.tok(1).is_some_and(|t| matches!(t.kind, TokenKind::Number | TokenKind::Param)))
            {
                self.start(Kind::Limit);
                self.bump();
                self.top_operand();
                self.bump_if_kw("PERCENT");
                if self.kw(0, "WITH") && self.kw(1, "TIES") {
                    self.bump();
                    self.bump();
                }
                self.finish();
            } else {
                break;
            }
        }
    }

    /// Was the token before the cursor the `DISTINCT` of a select (PostgreSQL rejects `SELECT DISTINCT FROM t`)?
    fn after_distinct(&self) -> bool {
        self.pos > 0 && self.sql[self.prev_span()].eq_ignore_ascii_case("DISTINCT")
    }

    /// MySQL lets an identifier start with digits (`1select`), which the lexer reads as a number followed by a
    /// word; such a pair is one name, not a stray word.
    fn glued_to_previous(&self, t: &crate::lexer::Token) -> bool {
        self.fam == Family::MySql && self.pos > 0 && self.prev_span().end == t.start
    }

    /// Is the next token one that ends a select list (or any list of expressions)?
    pub(super) fn at_terminator(&self) -> bool {
        self.is_terminator_at(0)
    }

    pub(super) fn is_terminator_at(&self, n: usize) -> bool {
        match self.tok(n) {
            None => true,
            Some(t) => match t.kind {
                TokenKind::Punct => matches!(self.text(&t), ")" | ";" | "," | "]"),
                TokenKind::Word => {
                    let w = self.text(&t);
                    words::cannot_start_expr(self.fam, w)
                        && !(self.punct(n + 1, "(") && words::is_function_name_ok(w))
                }
                _ => false,
            },
        }
    }

    /// Dialects that reject a comma with nothing after it in a select list, `GROUP BY`, `ORDER BY` or `FROM`
    /// (DuckDB accepts them; ClickHouse is not sure enough to say).
    pub(super) fn rejects_trailing_comma(&self) -> bool {
        matches!(self.fam, Family::Postgres | Family::MySql | Family::Sqlite | Family::SqlServer)
    }

    fn select_list(&mut self) {
        self.start(Kind::SelectList);
        let mut count = 0;
        loop {
            if self.punct(0, ",") {
                // `SELECT , a` and `SELECT a,, b`: an item is missing in every dialect.
                self.error_here("Unexpected comma: a select item is missing");
                self.start(Kind::Unknown);
                self.bump();
                self.finish();
                continue;
            }
            if self.expr_can_start() || self.op(0, "*") {
                self.select_item();
                count += 1;
            } else if count == 0 {
                // The empty list is valid SQL in PostgreSQL (`SELECT FROM t`, `SELECT;`) and DuckDB.
                if (!self.fam.pg_like() || self.after_distinct()) && self.at_terminator() {
                    self.error_here("Expected a select list");
                }
                break;
            } else {
                break;
            }
            if self.punct(0, ",") {
                self.bump();
                if self.at_terminator() && !self.punct(0, ",") {
                    // `SELECT a, FROM t`: PostgreSQL, MySQL, SQLite and T-SQL reject the trailing comma,
                    // DuckDB accepts it (and ClickHouse is not sure enough to say).
                    if self.rejects_trailing_comma() {
                        self.error_here("Trailing comma in the select list");
                    }
                    break;
                }
                continue;
            }
            break;
        }
        self.finish();
    }

    fn select_item(&mut self) {
        self.start(Kind::SelectItem);
        let outer_item = self.sel_item_depth.replace(self.depth);
        // T-SQL: `alias = expr`
        if self.fam == Family::SqlServer
            && self.tok(0).is_some_and(|t| {
                matches!(
                    t.kind,
                    TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String | TokenKind::Param
                )
            })
            && self.op(1, "=")
        {
            self.start(Kind::Alias);
            self.bump();
            self.finish();
            self.bump();
        }
        let mut star = false;
        if self.op(0, "*") {
            self.start(Kind::Star);
            self.bump();
            self.finish();
            star = true;
        } else if !self.expr() {
            // expr_can_start said yes but nothing was parsed (depth guard): keep the token
            self.start(Kind::Unknown);
            self.bump();
            self.finish();
        } else if self.last_child_is_star() {
            star = true;
        }
        if star {
            self.star_modifiers();
        } else {
            self.alias_in(true);
        }
        self.sel_item_depth = outer_item;
        self.select_item_leftover();
        self.finish();
    }

    /// Is the expression being parsed at `depth` the top-level expression of a select item?
    pub(super) fn in_select_item(&self, depth: u32) -> bool {
        self.sel_item_depth.is_some_and(|d| d + 1 == depth)
    }

    fn star_modifiers(&mut self) {
        loop {
            let ch = self.fam == Family::ClickHouse;
            if self.kw_any(0, &["EXCLUDE", "RENAME"]) && self.fam == Family::DuckDb
                || (self.kw(0, "REPLACE") && self.punct(1, "("))
                || (ch && self.kw_any(0, &["APPLY"]) && self.punct(1, "("))
                || (ch && self.kw(0, "EXCEPT") && self.punct(1, "("))
                || (self.kw(0, "ILIKE") && self.fam == Family::DuckDb)
            {
                self.start(Kind::Unknown);
                self.bump();
                if self.punct(0, "(") {
                    self.skip_group();
                } else {
                    self.bump();
                }
                self.finish();
            } else {
                break;
            }
        }
    }

    /// `[AS] alias [(columns)]` after a select item or a table factor. Returns whether one was consumed.
    pub(super) fn alias(&mut self) -> bool {
        self.alias_in(false)
    }

    /// May `w` be used as an alias without `AS`? Words that start a table suffix (`USE INDEX`, `PIVOT (`,
    /// `LOCK IN SHARE MODE`) are aliases when what follows does not continue that syntax.
    pub(super) fn bare_alias_ok(&self, w: &str) -> bool {
        if words::cannot_be_bare_alias(self.fam, w) {
            return false;
        }
        let up = w.to_ascii_uppercase();
        match up.as_str() {
            "PIVOT" | "UNPIVOT" | "PARTITION" | "FILTER" => !self.punct(1, "("),
            "USE" | "FORCE" | "IGNORE" => !self.kw_any(1, &["INDEX", "KEY"]),
            "FINAL" | "SAMPLE" | "FORMAT" => self.fam != Family::ClickHouse,
            "LOCK" => !self.kw(1, "IN"),
            "START" => !self.kw(1, "WITH"),
            "WITHIN" => !self.kw(1, "GROUP"),
            "AT" => !self.kw_any(1, &["TIME", "LOCAL"]),
            _ => true,
        }
    }

    /// `select_item`: a column label (PostgreSQL accepts far more words there than as a table alias).
    pub(super) fn alias_in(&mut self, select_item: bool) -> bool {
        let as_kw = self.kw(0, "AS");
        let ok = if as_kw {
            self.tok(1).is_some_and(|t| {
                matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String)
            })
        } else {
            self.tok(0).is_some_and(|t| match t.kind {
                TokenKind::Word => {
                    let w = self.text(&t);
                    if self.fam == Family::Postgres && select_item {
                        // Nearly every word is a bare label in PostgreSQL, unless it starts the next clause.
                        !words::pg_not_bare_label(w) && !self.join_ahead()
                    } else {
                        self.bare_alias_ok(w) && !self.join_ahead()
                    }
                }
                TokenKind::QuotedIdent => true,
                // `SELECT 1 'one'` is an alias in MySQL, SQLite and ClickHouse.
                TokenKind::String => !self.fam.pg_like(),
                _ => false,
            })
        };
        if !ok {
            return false;
        }
        self.start(Kind::Alias);
        if as_kw {
            self.bump();
        }
        self.bump();
        if self.punct(0, "(") && !self.paren_starts_query(0) {
            self.column_list();
        }
        self.finish();
        true
    }

    fn last_child_is_star(&mut self) -> bool {
        // The expression parser turns `t.*` into a Star node; peek at the frame's last child.
        self.last_node_kind() == Some(Kind::Star)
    }

    /// Tokens after a complete select item that are neither `,` nor the start of the next clause.
    fn select_item_leftover(&mut self) {
        // DuckDB is left out: it accepts `SELECT * fro` (a star followed by a word), which PostgreSQL does not.
        let strict = matches!(self.fam, Family::Postgres | Family::MySql | Family::Sqlite);
        let mut flagged = false;
        let mut started = false;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), "," | ")" | ";") {
                break;
            }
            if t.kind == TokenKind::Word {
                let w = self.text(&t);
                if words::cannot_start_expr(self.fam, w)
                    && !(self.punct(1, "(") && words::is_function_name_ok(w))
                {
                    break;
                }
                if self.kw_any(0, &["UNION", "WINDOW", "LOCK", "FINAL", "SAMPLE", "USE", "PARTITION"]) {
                    break;
                }
            }
            if !started {
                started = true;
                self.start(Kind::Unknown);
                if t.kind == TokenKind::Punct && self.text(&t) == "." {
                    self.error(t.start..t.end, "Unexpected '.'", false);
                }
                // Two aliases in a row (`SELECT a b c`), or a word where a comma was forgotten, is a syntax
                // error in these dialects. Operators, parentheses and strings are left alone: they are where
                // a construct this parser does not model most likely lives.
                if strict
                    && !flagged
                    && matches!(t.kind, TokenKind::Word | TokenKind::Number)
                    && !self.glued_to_previous(&t)
                {
                    flagged = true;
                    let text = self.text(&t).to_string();
                    self.error(t.start..t.end, format!("Unexpected '{text}'; expected ',' or FROM"), false);
                }
            }
            if self.punct(0, "(") {
                self.skip_group();
            } else {
                self.bump();
            }
        }
        if started {
            self.finish();
        }
    }

    // ---- clauses after the select list

    fn select_clauses(&mut self) {
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if t.kind != TokenKind::Word {
                self.junk();
                continue;
            }
            if self.kw(0, "FROM") {
                self.parse_from();
            } else if self.kw_any(0, &["WHERE", "PREWHERE"]) {
                self.where_clause();
            } else if self.kw(0, "GROUP") && self.kw(1, "BY") {
                self.group_by();
            } else if self.kw(0, "HAVING") {
                self.simple_clause(Kind::Having, "HAVING");
            } else if self.kw(0, "QUALIFY") {
                self.simple_clause(Kind::Qualify, "QUALIFY");
            } else if self.kw(0, "WINDOW") {
                self.window_clause();
            } else if self.kw(0, "INTO") {
                self.parse_into();
            } else if self.kw_any(
                0,
                &["UNION", "INTERSECT", "EXCEPT", "MINUS", "ORDER", "LIMIT", "OFFSET", "FETCH", "RETURNING"],
            ) || self.is_tail_start()
                || (self.kw(0, "ON") && self.kw_any(1, &["CONFLICT", "DUPLICATE"]))
            {
                break;
            } else {
                self.junk();
            }
        }
    }

    /// Words that start the dialect-specific tail of a query.
    pub(super) fn is_tail_start(&self) -> bool {
        self.kw_any(0, &["FOR", "LOCK", "OPTION", "SETTINGS", "FORMAT"])
            || (self.kw(0, "USING") && self.kw(1, "SAMPLE"))
            || (self.kw(0, "WITH")
                && self.kw_any(
                    1,
                    &["TIES", "TOTALS", "ROLLUP", "CUBE", "CHECK", "CASCADED", "LOCAL", "NO", "DATA"],
                ))
    }

    /// Skip unrecognised tokens (silently) up to the next token that can begin a clause.
    fn junk(&mut self) {
        // A plain word or number where a clause keyword or the end belongs (`FROM t wher x`, `LIMIT 1 2`) is
        // an error in PostgreSQL, MySQL and SQLite. Other dialects have more trailing syntax than this
        // parser models, so they stay silent.
        if matches!(self.fam, Family::Postgres | Family::MySql | Family::Sqlite)
            && let Some(t) = self.peek()
            && matches!(t.kind, TokenKind::Word | TokenKind::Number)
            && !self.glued_to_previous(&t)
        {
            let text = self.text(&t).to_string();
            self.error(t.start..t.end, format!("Unexpected '{text}'"), false);
        }
        self.start(Kind::Unknown);
        let mut first = true;
        let mut depth = 0i32;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => {
                        self.skip_group();
                        first = false;
                        continue;
                    }
                    ")" if depth == 0 => break,
                    ";" => break,
                    _ => {}
                }
            }
            if !first && t.kind == TokenKind::Word {
                let w = self.text(&t);
                if [
                    "FROM",
                    "WHERE",
                    "GROUP",
                    "HAVING",
                    "WINDOW",
                    "QUALIFY",
                    "UNION",
                    "INTERSECT",
                    "EXCEPT",
                    "MINUS",
                    "ORDER",
                    "LIMIT",
                    "OFFSET",
                    "FETCH",
                    "RETURNING",
                ]
                .iter()
                .any(|k| w.eq_ignore_ascii_case(k))
                {
                    break;
                }
            }
            first = false;
            depth = depth.max(0);
            self.bump();
        }
        self.finish();
    }

    fn parse_into(&mut self) {
        self.start(Kind::IntoClause);
        self.bump();
        self.bump_if_kw("TEMP");
        self.bump_if_kw("TEMPORARY");
        self.bump_if_kw("UNLOGGED");
        self.bump_if_kw("TABLE");
        if self.at_name_start() {
            self.table_name();
        }
        // `INTO @a, @b`, `INTO OUTFILE 'f' ...`: keep the rest up to the next clause.
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if t.kind == TokenKind::Word
                && self.kw_any(
                    0,
                    &["FROM", "WHERE", "GROUP", "HAVING", "UNION", "ORDER", "LIMIT", "WINDOW", "FOR"],
                )
            {
                break;
            }
            if self.punct(0, "(") {
                self.skip_group();
            } else {
                self.bump();
            }
        }
        self.finish();
    }

    pub(super) fn where_clause(&mut self) {
        self.start(Kind::WhereClause);
        self.bump();
        if self.kw(0, "CURRENT") && self.kw(1, "OF") {
            self.bump();
            self.bump();
            self.bump();
        } else {
            self.expect_expr("WHERE");
        }
        self.finish();
    }

    fn simple_clause(&mut self, kind: Kind, what: &str) {
        self.start(kind);
        self.bump();
        self.expect_expr(what);
        self.finish();
    }

    fn window_clause(&mut self) {
        self.start(Kind::WindowClause);
        self.bump();
        loop {
            if !self.is_ident(0) {
                break;
            }
            self.bump();
            self.bump_if_kw("AS");
            if self.punct(0, "(") {
                self.window_def();
            }
            if self.punct(0, ",") {
                self.bump();
            } else {
                break;
            }
        }
        self.finish();
    }

    fn group_by(&mut self) {
        self.start(Kind::GroupBy);
        self.bump();
        self.bump();
        let mut first = true;
        if self.kw_any(0, &["ALL", "DISTINCT"]) && !self.punct(1, "(") {
            self.bump();
            // DuckDB's `GROUP BY ALL` stands alone.
            first = false;
        }
        loop {
            if self.kw(0, "GROUPING") && self.kw(1, "SETS") && self.punct(2, "(") {
                self.bump();
                self.bump();
                self.arg_list_for("");
            } else if self.expr_can_start() {
                self.expr();
            } else {
                if first || self.punct(0, ",") {
                    // `GROUP BY` with no expression, or `GROUP BY a,` : never valid.
                }
                if first && self.at_terminator() {
                    self.error_here("Expected an expression after GROUP BY");
                }
                break;
            }
            first = false;
            if self.punct(0, ",") {
                self.bump();
                if self.at_terminator() && !self.punct(0, ",") {
                    if self.rejects_trailing_comma() {
                        self.error_here("Expected an expression after the comma");
                    }
                    break;
                }
                continue;
            }
            break;
        }
        // `WITH ROLLUP` (MySQL), `WITH TOTALS` (ClickHouse)
        if self.kw(0, "WITH") && self.kw_any(1, &["ROLLUP", "CUBE", "TOTALS"]) {
            self.bump();
            self.bump();
        }
        self.finish();
    }

    // ---- query tail: ORDER BY, LIMIT, locking

    pub(super) fn query_tail(&mut self) {
        loop {
            if self.kw(0, "ORDER") && self.kw(1, "BY") {
                self.order_by();
            } else if self.kw_any(0, &["LIMIT", "OFFSET", "FETCH"]) {
                self.limit_clause();
            } else if self.is_tail_start() || self.kw(0, "INTO") {
                self.tail();
            } else {
                break;
            }
        }
    }

    fn tail(&mut self) {
        self.start(Kind::Tail);
        if self.kw_any(0, &["FOR"]) && self.tok(1).is_none() && self.fam != Family::Sqlite {
            self.bump();
            self.error_here("Expected UPDATE, SHARE or similar after FOR");
        }
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if self.punct(0, "(") {
                self.skip_group();
            } else {
                self.bump();
            }
        }
        self.finish();
    }

    pub(super) fn order_by(&mut self) {
        self.start(Kind::OrderBy);
        self.bump();
        self.bump_if_kw("SIBLINGS");
        self.bump(); // BY
        let mut first = true;
        loop {
            if self.kw(0, "ALL") && !self.punct(1, "(") {
                self.bump();
            } else if self.expr_can_start() {
                self.order_item();
            } else {
                if first && self.at_terminator() {
                    self.error_here("Expected an expression after ORDER BY");
                }
                break;
            }
            first = false;
            if self.punct(0, ",") {
                self.bump();
                if self.at_terminator() && !self.punct(0, ",") {
                    if self.rejects_trailing_comma() {
                        self.error_here("Expected an expression after the comma");
                    }
                    break;
                }
                continue;
            }
            break;
        }
        self.finish();
    }

    fn order_item(&mut self) {
        self.start(Kind::OrderItem);
        self.expr();
        self.bump_if_kw("ASC");
        self.bump_if_kw("DESC");
        if self.kw(0, "NULLS") && self.kw_any(1, &["FIRST", "LAST"]) {
            self.bump();
            self.bump();
        } else if self.kw(0, "NULLS") && self.is_terminator_at(1) {
            self.bump();
            self.error_here("Expected FIRST or LAST after NULLS");
        }
        if self.kw(0, "USING") && self.tok(1).is_some_and(|t| t.kind == TokenKind::Operator) {
            self.bump();
            self.bump();
        }
        self.finish();
    }

    fn limit_clause(&mut self) {
        self.start(Kind::Limit);
        let what = if self.kw(0, "LIMIT") {
            "LIMIT"
        } else if self.kw(0, "OFFSET") {
            "OFFSET"
        } else {
            "FETCH"
        };
        self.bump();
        if what == "FETCH" {
            if self.at_end() && matches!(self.fam, Family::Postgres | Family::SqlServer) {
                self.error_here("Expected FIRST or NEXT after FETCH");
            }
            self.bump_if_kw("FIRST");
            self.bump_if_kw("NEXT");
            if !self.kw_any(0, &["ROW", "ROWS"]) {
                self.expr();
            }
            while self.kw_any(0, &["ROW", "ROWS", "PERCENT", "ONLY", "WITH", "TIES"]) {
                self.bump();
            }
        } else {
            self.bump_if_kw("ALL");
            // SQLite lets `OFFSET` stand as a column alias (`SELECT 1 OFFSET`), so a bare one proves nothing.
            if !self.kw_any(0, &["BY"])
                && !(what == "OFFSET" && self.fam == Family::Sqlite && self.at_terminator())
            {
                self.expect_expr(what);
            }
            if self.punct(0, ",") && what == "LIMIT" {
                self.bump();
                self.expect_expr("the comma");
            }
            if what == "OFFSET" {
                self.bump_if_kw("ROW");
                self.bump_if_kw("ROWS");
            }
            // ClickHouse `LIMIT n BY expr, ...`
            if self.kw(0, "BY") && self.fam == Family::ClickHouse {
                self.bump();
                while self.expr_can_start() {
                    self.expr();
                    if !self.bump_if_punct(",") {
                        break;
                    }
                }
            }
        }
        self.finish();
    }

    // ---- FROM

    pub(super) fn parse_from(&mut self) {
        self.start(Kind::FromClause);
        self.bump();
        self.table_ref_list("FROM");
        self.finish();
    }

    /// `table_ref {, table_ref}`; reports a missing first table where that is certain.
    pub(super) fn table_ref_list(&mut self, after: &str) {
        let mut first = true;
        loop {
            if !self.table_ref() {
                if first && self.at_terminator() {
                    self.error_here(format!("Expected a table after {after}"));
                } else if !first
                    && self.at_terminator()
                    && self.prev_is_comma()
                    && self.rejects_trailing_comma()
                {
                    self.error_here("Expected a table after the comma");
                }
                break;
            }
            first = false;
            if self.punct(0, ",") {
                self.bump();
                continue;
            }
            break;
        }
    }

    fn prev_is_comma(&self) -> bool {
        let p = self.prev_span();
        &self.sql[p] == ","
    }

    /// A table factor followed by any number of joins.
    pub(super) fn table_ref(&mut self) -> bool {
        let m = self.mark();
        if !self.table_factor() {
            return false;
        }
        while self.join_ahead() {
            self.join(m);
        }
        true
    }

    /// Is a join (modifier words then `JOIN`, `APPLY`, or `ARRAY JOIN`) next?
    pub(super) fn join_ahead(&self) -> bool {
        if self.kw(0, "STRAIGHT_JOIN") {
            return true;
        }
        let mut k = 0;
        while k < 5 && self.tok(k).is_some_and(|t| t.kind == TokenKind::Word) {
            let t = self.tok(k).unwrap();
            let w = self.text(&t);
            if w.eq_ignore_ascii_case("JOIN") || w.eq_ignore_ascii_case("APPLY") {
                return true;
            }
            if w.eq_ignore_ascii_case("ARRAY") && self.kw(k + 1, "JOIN") {
                return true;
            }
            if !words::is_join_modifier(w) {
                return false;
            }
            k += 1;
        }
        false
    }

    fn join(&mut self, m: usize) {
        let mut cross_like = false;
        let mut apply = false;
        let mut array_join = false;
        while let Some(t) = self.peek() {
            let w = self.text(&t).to_ascii_uppercase();
            self.bump();
            match w.as_str() {
                "JOIN" | "STRAIGHT_JOIN" => break,
                "APPLY" => {
                    apply = true;
                    break;
                }
                "CROSS" | "NATURAL" | "ASOF" | "POSITIONAL" => cross_like = true,
                "ARRAY" => array_join = true,
                _ => {}
            }
        }
        if array_join {
            // ClickHouse `ARRAY JOIN expr [AS alias], ...`
            loop {
                if !self.expr_can_start() {
                    break;
                }
                self.expr();
                self.alias();
                if !self.bump_if_punct(",") {
                    break;
                }
            }
            self.wrap_merging(m, Kind::Join, &|_| true);
            return;
        }
        let m_right = self.mark();
        if !self.table_factor() && self.at_terminator() {
            self.error_here("Expected a table after JOIN");
        }
        // `a JOIN b JOIN c ON x ON y`: the first ON belongs to the inner join.
        while !cross_like && self.join_ahead() && !self.kw_any(0, &["ON", "USING"]) {
            self.join(m_right);
        }
        if self.kw(0, "ON") {
            self.start(Kind::JoinConstraint);
            self.bump();
            self.expect_expr("ON");
            self.finish();
        } else if self.kw(0, "USING") && self.punct(1, "(") {
            self.start(Kind::JoinConstraint);
            self.bump();
            self.column_list();
            // PostgreSQL's `USING (a) AS j`
            if self.kw(0, "AS") && self.is_ident(1) {
                self.bump();
                self.bump();
            }
            self.finish();
        } else if !cross_like
            && !apply
            && self.at_terminator()
            && !self.punct(0, ",")
            && matches!(self.fam, Family::Postgres | Family::SqlServer)
        {
            // PostgreSQL's and T-SQL's grammar require a join condition for every join that is not CROSS or
            // NATURAL (MySQL and SQLite treat a bare JOIN as a cross join; DuckDB is not covered here).
            self.error_here("JOIN requires an ON or USING condition");
        }
        self.wrap_merging(m, Kind::Join, &|_| true);
    }

    /// Is the `.` at the cursor followed by something that cannot continue a name (an operator, a clause
    /// word, `)`, the end)?
    fn at_name_end_after_dot(&self) -> bool {
        match self.tok(1) {
            None => true,
            Some(t) => match t.kind {
                TokenKind::Punct => matches!(self.text(&t), ")" | "," | ";"),
                TokenKind::Operator => true,
                TokenKind::Word => self.is_terminator_at(1),
                _ => false,
            },
        }
    }

    fn at_end_after(&self, n: usize) -> bool {
        self.tok(n).is_none()
    }

    /// Can a table name (possibly `#temp`) start at the cursor?
    pub(super) fn at_name_start(&self) -> bool {
        if self.is_ident(0) {
            return true;
        }
        self.fam == Family::SqlServer && self.hash_prefix_len().is_some()
    }

    /// T-SQL `#tmp` / `##tmp`: operator token(s) glued to an identifier.
    fn hash_prefix_len(&self) -> Option<usize> {
        let t = self.tok(0)?;
        if t.kind == TokenKind::Operator && matches!(self.text(&t), "#" | "##") {
            let n = self.tok(1)?;
            if n.start == t.end && matches!(n.kind, TokenKind::Word | TokenKind::QuotedIdent) {
                return Some(1);
            }
        }
        None
    }

    /// A dotted name into a `Name` node.
    pub(super) fn table_name(&mut self) {
        self.start(Kind::Name);
        self.name_tokens(false);
        self.finish();
    }

    /// Identifier tokens joined by dots into the current node. Returns whether the name ended in `.*`.
    pub(super) fn name_tokens(&mut self, allow_star: bool) -> bool {
        if self.hash_prefix_len().is_some() {
            self.bump();
        }
        if self.is_ident(0) {
            self.bump();
        }
        let mut star = false;
        while self.punct(0, ".")
            && (self.is_ident(1) || self.punct(1, ".") || (allow_star && self.op(1, "*")))
        {
            self.bump();
            if self.is_ident(0) {
                self.bump();
            } else if allow_star && self.op(0, "*") {
                self.bump();
                star = true;
                break;
            }
        }
        if self.punct(0, ".") && self.at_name_end_after_dot() {
            // `t.` with nothing after the dot.
            let sp = self.next_span();
            self.error(sp, "Expected a name after '.'", self.at_end_after(1));
            self.start(Kind::Unknown);
            self.bump();
            self.finish();
        }
        star
    }

    pub(super) fn table_factor(&mut self) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::Punct if self.text(&t) == "(" => {
                if self.paren_starts_query(0) {
                    self.derived_table(false);
                } else {
                    self.paren_from();
                }
                true
            }
            TokenKind::Word | TokenKind::QuotedIdent => {
                let w = self.text(&t);
                if t.kind == TokenKind::Word {
                    if self.kw(0, "LATERAL") && (self.punct(1, "(") || self.is_ident(1)) {
                        if self.paren_starts_query(1) {
                            self.derived_table(true);
                            return true;
                        }
                        let m = self.mark();
                        self.bump();
                        if !self.table_factor() {
                            return false;
                        }
                        self.wrap(m, Kind::TableFunc);
                        return true;
                    }
                    if self.kw(0, "VALUES") && !self.punct(1, "(") {
                        return false;
                    }
                    if words::cannot_start_expr(self.fam, w)
                        && !(self.punct(1, "(") && words::is_function_name_ok(w))
                    {
                        return false;
                    }
                    if self.kw(0, "ONLY") && (self.is_ident(1) || self.punct(1, "(")) {
                        let m = self.mark();
                        self.bump();
                        let paren = self.bump_if_punct("(");
                        let ok = self.table_name_or_func();
                        if paren {
                            self.bump_if_punct(")");
                        }
                        let _ = m;
                        return ok;
                    }
                }
                self.table_name_or_func()
            }
            TokenKind::String if self.fam == Family::DuckDb => {
                // DuckDB's replacement scan: `FROM 'data.parquet'`.
                let m = self.mark();
                self.start(Kind::Literal);
                self.bump();
                self.finish();
                self.alias();
                self.wrap(m, Kind::TableFunc);
                true
            }
            TokenKind::Param => {
                let m = self.mark();
                self.start(Kind::Param);
                self.bump();
                self.finish();
                self.alias();
                self.wrap(m, Kind::TableFunc);
                true
            }
            TokenKind::Operator if self.hash_prefix_len().is_some() => self.table_name_or_func(),
            _ => false,
        }
    }

    fn table_name_or_func(&mut self) -> bool {
        if !self.at_name_start() {
            return false;
        }
        let m = self.mark();
        self.table_name();
        if self.punct(0, "(") {
            self.arg_list_for("");
            self.wrap(m, Kind::FuncCall);
            if self.kw(0, "WITH") && self.kw(1, "ORDINALITY") {
                self.bump();
                self.bump();
            }
            self.alias();
            self.table_suffixes();
            self.wrap(m, Kind::TableFunc);
        } else {
            self.alias();
            self.table_suffixes();
            self.wrap(m, Kind::TableName);
        }
        true
    }

    /// Hints and modifiers after a table: `WITH (NOLOCK)`, `USE INDEX (i)`, `PARTITION (p)`, `TABLESAMPLE`,
    /// `INDEXED BY i`, `FINAL`, `SAMPLE`, `PIVOT (...)`. Kept as tokens.
    fn table_suffixes(&mut self) {
        loop {
            if self.kw(0, "WITH") && self.punct(1, "(") {
                self.bump();
                self.skip_group();
            } else if self.kw_any(0, &["USE", "FORCE", "IGNORE"]) && self.kw_any(1, &["INDEX", "KEY"]) {
                self.bump();
                self.bump();
                if self.kw(0, "FOR") {
                    self.bump();
                    self.bump();
                    self.bump_if_kw("BY");
                }
                if self.punct(0, "(") {
                    self.skip_group();
                }
            } else if self.kw(0, "PARTITION") && self.punct(1, "(") {
                self.bump();
                self.skip_group();
            } else if self.kw(0, "TABLESAMPLE") {
                self.bump();
                self.bump_if_word();
                if self.punct(0, "(") {
                    self.skip_group();
                }
                if self.kw(0, "REPEATABLE") && self.punct(1, "(") {
                    self.bump();
                    self.skip_group();
                }
            } else if self.kw(0, "INDEXED") && self.kw(1, "BY") {
                self.bump();
                self.bump();
                self.bump();
            } else if self.kw(0, "NOT") && self.kw(1, "INDEXED") {
                self.bump();
                self.bump();
            } else if self.kw(0, "FINAL") && self.fam == Family::ClickHouse {
                self.bump();
            } else if self.kw(0, "SAMPLE") && self.fam == Family::ClickHouse {
                self.bump();
                self.bump();
            } else if self.kw_any(0, &["PIVOT", "UNPIVOT"]) && self.punct(1, "(") {
                self.bump();
                self.skip_group();
                self.alias();
            } else {
                break;
            }
        }
    }

    fn bump_if_word(&mut self) -> bool {
        if self.is_word_tok(0) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn derived_table(&mut self, lateral: bool) {
        self.start(Kind::DerivedTable);
        if lateral {
            self.bump();
        }
        let open = self.next_span();
        self.bump();
        self.query();
        self.expect_close(open);
        self.alias();
        self.table_suffixes();
        self.finish();
    }

    fn paren_from(&mut self) {
        self.start(Kind::ParenFrom);
        let open = self.next_span();
        self.bump();
        if self.punct(0, ")") {
            self.error_here("Expected a table inside the parentheses");
        }
        loop {
            if !self.table_ref() {
                break;
            }
            if !self.bump_if_punct(",") {
                break;
            }
        }
        self.expect_close(open);
        self.alias();
        self.finish();
    }
}
