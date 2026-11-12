//! INSERT, UPDATE, DELETE, MERGE (parsed in depth) and DDL (shallow: the kind and the objects it names).

use super::node::{Kind, Node};
use super::parser::Parser;
use crate::lexer::TokenKind;
use theta_core::Family;

const STARTERS_DML: &[&str] = &[
    "VALUES",
    "VALUE",
    "SELECT",
    "WITH",
    "SET",
    "FROM",
    "WHERE",
    "RETURNING",
    "OUTPUT",
    "ON",
    "USING",
    "ORDER",
    "LIMIT",
    "DEFAULT",
    "OVERRIDING",
    "TABLE",
    "FORMAT",
    "SETTINGS",
    "BY",
    "EXEC",
    "EXECUTE",
];

impl Parser<'_> {
    /// Skip unrecognised tokens in a DML statement up to the next clause keyword (silently).
    fn dml_junk(&mut self) {
        self.start(Kind::Unknown);
        let mut first = true;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => {
                        self.skip_group();
                        first = false;
                        continue;
                    }
                    ")" | ";" if !first => break,
                    ")" => {
                        // A `)` that closes nothing here: left for the statement-level check.
                        break;
                    }
                    _ => {}
                }
            }
            if !first && t.kind == TokenKind::Word {
                let w = self.text(&t);
                if STARTERS_DML.iter().any(|k| w.eq_ignore_ascii_case(k)) {
                    break;
                }
            }
            first = false;
            self.bump();
        }
        self.finish();
    }

    // ---- INSERT

    pub(super) fn insert(&mut self, with: Option<Node>) {
        self.start_adopting(Kind::Insert, with);
        self.bump(); // INSERT / REPLACE / UPSERT
        loop {
            if self.kw(0, "OR") && self.is_word_tok(1) {
                self.bump();
                self.bump();
            } else if self.kw_any(0, &["LOW_PRIORITY", "DELAYED", "HIGH_PRIORITY", "IGNORE"]) {
                self.bump();
            } else {
                break;
            }
        }
        if !self.bump_if_kw("INTO")
            && matches!(self.fam, Family::Postgres | Family::Sqlite | Family::DuckDb)
            && self.tok(0).is_some()
        {
            // PostgreSQL, SQLite and DuckDB require INTO (MySQL and T-SQL make it optional).
            self.error_here("Expected INTO after INSERT");
        }
        self.bump_if_kw("TABLE");
        let mut has_source = false;
        if self.at_name_start() {
            self.table_name();
        } else if self.tok(0).is_some_and(|t| t.kind == TokenKind::Param) {
            self.start(Kind::Param);
            self.bump();
            self.finish();
        } else if self.kw(0, "FUNCTION") {
            // ClickHouse `INSERT INTO FUNCTION ...`: opaque
            has_source = true;
        } else if self.at_terminator() || (self.punct(0, "(") && !self.paren_starts_query(0)) {
            self.error_here("Expected a table name after INSERT INTO");
        }
        if self.kw(0, "AS") && self.is_ident(1) {
            self.start(Kind::Alias);
            self.bump();
            self.bump();
            self.finish();
        }
        if self.punct(0, "(") && !self.paren_starts_query(0) {
            self.column_list();
        }
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if self.kw(0, "OVERRIDING") {
                while self.kw_any(0, &["OVERRIDING", "SYSTEM", "USER", "VALUE"]) {
                    self.bump();
                }
            } else if self.kw_any(0, &["OUTPUT", "RETURNING"]) {
                self.returning();
            } else if self.kw(0, "WITH") && self.punct(1, "(") {
                self.bump();
                self.skip_group();
            } else if self.kw(0, "DEFAULT") && self.kw(1, "VALUES") {
                has_source = true;
                self.bump();
                self.bump();
            } else if self.kw_any(0, &["VALUES", "VALUE"]) {
                has_source = true;
                self.values_body();
            } else if self.kw(0, "SET") {
                has_source = true;
                self.set_clause();
            } else if self.kw(0, "BY") && self.kw_any(1, &["NAME", "POSITION"]) {
                self.bump();
                self.bump();
            } else if self.is_query_start(0) || self.paren_starts_query(0) {
                has_source = true;
                self.query();
            } else if self.kw(0, "ON") && self.kw(1, "CONFLICT") {
                self.on_conflict();
            } else if self.kw(0, "ON") && self.kw(1, "DUPLICATE") {
                self.on_duplicate();
            } else {
                has_source = true; // something follows the target (`EXEC proc`, `FORMAT CSV`): not ours to judge
                self.dml_junk();
            }
        }
        if !has_source && self.at_end() && self.fam != Family::ClickHouse {
            // `INSERT INTO t (a)` with nothing to insert.
            self.error_here("Expected VALUES or a query in INSERT");
        }
        self.finish();
    }

    fn on_conflict(&mut self) {
        self.start(Kind::OnConflict);
        self.bump();
        self.bump();
        if self.at_end() {
            self.error_here("Expected DO NOTHING or DO UPDATE after ON CONFLICT");
        }
        if self.punct(0, "(") {
            self.column_list();
        }
        if self.kw(0, "ON") && self.kw(1, "CONSTRAINT") {
            self.bump();
            self.bump();
            self.bump();
        }
        if self.kw(0, "WHERE") {
            self.where_clause();
        }
        if !self.kw(0, "DO") && self.at_end() {
            self.error_here("Expected DO NOTHING or DO UPDATE after ON CONFLICT");
        }
        if self.kw(0, "DO") {
            self.bump();
            if self.at_end() {
                self.error_here("Expected NOTHING or UPDATE after DO");
            }
            if self.kw(0, "NOTHING") {
                self.bump();
            } else if self.kw(0, "UPDATE") {
                self.bump();
                if self.kw(0, "SET") {
                    self.set_clause();
                } else if self.at_end() {
                    self.error_here("Expected SET after DO UPDATE");
                }
                if self.kw(0, "WHERE") {
                    self.where_clause();
                }
            }
        }
        self.finish();
    }

    fn on_duplicate(&mut self) {
        self.start(Kind::OnDuplicate);
        self.bump(); // ON
        self.bump(); // DUPLICATE
        self.bump_if_kw("KEY");
        if !self.bump_if_kw("UPDATE") && self.at_end() {
            self.error_here("Expected UPDATE after ON DUPLICATE KEY");
        }
        self.assignments();
        self.finish();
    }

    /// `RETURNING items` or T-SQL `OUTPUT items [INTO target [(cols)]]`.
    pub(super) fn returning(&mut self) {
        self.start(Kind::Returning);
        self.bump();
        if self.expr_can_start() || self.op(0, "*") {
            self.start(Kind::SelectList);
            loop {
                if self.expr_can_start() || self.op(0, "*") {
                    self.returning_item();
                } else {
                    break;
                }
                if !self.bump_if_punct(",") {
                    break;
                }
            }
            self.finish();
        } else if self.at_terminator() {
            self.error_here("Expected an expression after RETURNING");
        }
        if self.kw(0, "INTO") {
            self.bump();
            if self.at_name_start() {
                self.table_name();
            } else if self.tok(0).is_some_and(|t| t.kind == TokenKind::Param) {
                self.bump();
            }
            if self.punct(0, "(") {
                self.column_list();
            }
        }
        self.finish();
    }

    fn returning_item(&mut self) {
        self.start(Kind::SelectItem);
        if self.op(0, "*") {
            self.start(Kind::Star);
            self.bump();
            self.finish();
        } else {
            self.expr();
            self.alias_in(true);
        }
        self.finish();
    }

    // ---- SET / assignments

    pub(super) fn set_clause(&mut self) {
        self.start(Kind::SetClause);
        self.bump();
        self.assignments();
        self.finish();
    }

    fn assignments(&mut self) {
        let mut first = true;
        loop {
            if !self.assignment() {
                if first && self.at_end() {
                    self.error_here("Expected a column assignment");
                } else if !first && self.at_terminator() && self.rejects_trailing_comma() {
                    // `SET a = 1, WHERE ...`
                    self.error_here("Expected a column assignment after the comma");
                } else if self.op(0, "=") {
                    self.error_here("Expected a column before '='");
                }
                break;
            }
            first = false;
            if !self.bump_if_punct(",") {
                break;
            }
        }
    }

    fn assignment(&mut self) -> bool {
        if !(self.expr_can_start()) {
            if self.at_terminator() && !self.at_end() && self.punct(0, ",") {
                return false;
            }
            return false;
        }
        self.start(Kind::Assignment);
        // Target: a column (possibly qualified or indexed), a `(a, b)` list or a variable. Parsed as an
        // expression of the comparison level so `=` is left for us.
        self.assignment_target();
        if self.tok(0).is_some_and(|t| {
            t.kind == TokenKind::Operator
                && self.text(&t).ends_with('=')
                && !matches!(self.text(&t), "<=" | ">=" | "!=" | "==")
        }) {
            self.bump();
            if self.kw(0, "DEFAULT") {
                self.bump();
            } else if !self.expr() && self.at_terminator() {
                self.error_here("Expected a value after '='");
            }
        } else if self.at_terminator() && self.at_end() {
            // `SET col` with nothing after: the statement stops mid-assignment.
            self.error_here("Expected '=' after the column in SET");
        } else {
            // `SET a = 1` is the only form; anything else is not understood here.
            if self.engines_checked()
                && self.tok(0).is_some_and(|t| {
                    matches!(t.kind, TokenKind::Word | TokenKind::String | TokenKind::Number)
                        || (t.kind == TokenKind::Punct && self.text(&t) == ",")
                })
            {
                // The target is always followed by `=` in these four (`SET col 'x'`, `SET col WHERE`,
                // `SET a, b = 1`).
                self.error_here("Expected '=' after the column in SET");
            }
            self.start(Kind::Unknown);
            while let Some(t) = self.peek() {
                if t.kind == TokenKind::Punct && matches!(self.text(&t), "," | ")" | ";") {
                    break;
                }
                if self.kw_any(0, &["WHERE", "FROM", "RETURNING", "OUTPUT", "ORDER", "LIMIT"]) {
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
        self.finish();
        true
    }

    fn assignment_target(&mut self) {
        if self.punct(0, "(") {
            self.paren_target();
            return;
        }
        if self.tok(0).is_some_and(|t| t.kind == TokenKind::Param) {
            self.start(Kind::Param);
            self.bump();
            self.finish();
            return;
        }
        if self.is_ident(0) || self.at_name_start() {
            let m = self.mark();
            self.start(Kind::Name);
            self.name_tokens(false);
            self.finish();
            self.retag_last(Kind::ColumnRef);
            // `arr[1] = ..`, `col.field = ..`
            while self.punct(0, "[") {
                self.bump();
                while !self.at_end() && !self.punct(0, "]") {
                    if !self.expr() {
                        self.bump();
                    }
                }
                self.bump_if_punct("]");
                self.wrap(m, Kind::Index);
            }
        }
    }

    fn paren_target(&mut self) {
        let m = self.mark();
        let open = self.next_span();
        self.bump();
        loop {
            if self.at_end() || self.punct(0, ")") {
                break;
            }
            if self.punct(0, ",") {
                self.bump();
            } else if self.is_ident(0) {
                self.start(Kind::Name);
                self.name_tokens(false);
                self.finish();
                self.retag_last(Kind::ColumnRef);
            } else {
                self.bump();
            }
        }
        self.expect_close(open);
        self.wrap(m, Kind::RowCtor);
    }

    // ---- UPDATE

    pub(super) fn update(&mut self, with: Option<Node>) {
        self.start_adopting(Kind::Update, with);
        self.bump();
        loop {
            if self.kw(0, "OR") && self.is_word_tok(1) {
                self.bump();
                self.bump();
            } else if self.kw_any(0, &["LOW_PRIORITY", "IGNORE"]) {
                self.bump();
            } else if self.kw(0, "TOP") && self.punct(1, "(") {
                self.start(Kind::Limit);
                self.bump();
                self.top_operand();
                self.bump_if_kw("PERCENT");
                self.finish();
            } else {
                break;
            }
        }
        let mut seen_set = false;
        // Target tables (MySQL allows several and joins) up to SET.
        if self.kw(0, "ONLY") || self.table_ref_starts() {
            self.table_ref_list("UPDATE");
        } else if self.at_terminator() {
            self.error_here("Expected a table name after UPDATE");
        }
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if self.kw(0, "SET") {
                seen_set = true;
                self.set_clause();
            } else if self.kw_any(0, &["OUTPUT", "RETURNING"]) {
                self.returning();
            } else if self.kw(0, "FROM") {
                self.parse_from();
            } else if self.kw(0, "WHERE") {
                self.where_clause();
            } else if self.kw(0, "ORDER") && self.kw(1, "BY") {
                self.order_by();
            } else if self.kw(0, "LIMIT") {
                self.query_tail();
            } else {
                self.dml_junk();
            }
        }
        if !seen_set && self.at_end() {
            // An UPDATE without SET is not a statement in any dialect.
            self.error_here("UPDATE needs a SET clause");
        }
        self.finish();
    }

    fn table_ref_starts(&self) -> bool {
        self.at_name_start() && !self.kw(0, "SET") || self.punct(0, "(")
    }

    // ---- DELETE

    pub(super) fn delete(&mut self, with: Option<Node>) {
        self.start_adopting(Kind::Delete, with);
        self.bump();
        loop {
            if self.kw_any(0, &["LOW_PRIORITY", "QUICK", "IGNORE"]) {
                self.bump();
            } else if self.kw(0, "TOP") && self.punct(1, "(") {
                self.start(Kind::Limit);
                self.bump();
                self.top_operand();
                self.bump_if_kw("PERCENT");
                self.finish();
            } else {
                break;
            }
        }
        if self.kw(0, "FROM") {
            self.bump();
            if self.kw(0, "ONLY") || self.table_ref_starts() {
                // The target, with an optional alias; MySQL's `DELETE FROM t1, t2 USING ...` lists several.
                self.table_ref_list("DELETE FROM");
            } else if self.at_terminator() {
                self.error_here("Expected a table name after DELETE FROM");
            }
        } else if self.at_name_start() && !self.kw_any(0, &["WHERE", "OUTPUT"]) {
            // MySQL / T-SQL: `DELETE t1 FROM t1 JOIN ...`, `DELETE t WHERE ...`
            if matches!(self.fam, Family::Postgres | Family::Sqlite | Family::DuckDb) {
                // PostgreSQL, SQLite and DuckDB only have `DELETE FROM`.
                self.error_here("Expected FROM after DELETE");
            }
            self.table_ref_list("DELETE");
            if self.fam == Family::MySql && !self.kw(0, "FROM") && self.tok(0).is_some() {
                // MySQL's multi-table form is `DELETE t1 FROM t1 JOIN ...`; without a FROM it is not a statement
                // (T-SQL alone allows `DELETE t WHERE ...`).
                self.error_here("Expected FROM after the DELETE target");
            }
        }
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if self.kw(0, "USING") {
                self.start(Kind::UsingClause);
                self.bump();
                self.table_ref_list("USING");
                self.finish();
            } else if self.kw(0, "FROM") {
                self.parse_from();
            } else if self.kw(0, "WHERE") {
                self.where_clause();
            } else if self.kw_any(0, &["OUTPUT", "RETURNING"]) {
                self.returning();
            } else if self.kw(0, "ORDER") && self.kw(1, "BY") {
                self.order_by();
            } else if self.kw(0, "LIMIT") {
                self.query_tail();
            } else {
                self.dml_junk();
            }
        }
        self.finish();
    }

    // ---- MERGE

    pub(super) fn merge(&mut self, with: Option<Node>) {
        self.start_adopting(Kind::Merge, with);
        self.bump();
        if self.kw(0, "TOP") && self.punct(1, "(") {
            self.start(Kind::Limit);
            self.bump();
            self.top_operand();
            self.bump_if_kw("PERCENT");
            self.finish();
        }
        self.bump_if_kw("INTO");
        // target [AS alias]
        if self.table_factor() {
            // ok
        } else if self.at_terminator() {
            self.error_here("Expected a target table after MERGE");
        }
        if self.kw(0, "USING") {
            self.start(Kind::UsingClause);
            self.bump();
            if !self.table_ref() && self.at_terminator() {
                self.error_here("Expected a source after USING");
            }
            self.finish();
        } else if self.at_end() {
            self.error_here("Expected USING in MERGE");
        }
        if self.kw(0, "ON") {
            self.start(Kind::JoinConstraint);
            self.bump();
            self.expect_expr("ON");
            self.finish();
        } else if self.at_end() {
            self.error_here("Expected ON in MERGE");
        }
        let mut whens = 0;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct && matches!(self.text(&t), ")" | ";") {
                break;
            }
            if self.kw(0, "WHEN") {
                whens += 1;
                self.merge_clause();
            } else if self.kw_any(0, &["OUTPUT", "RETURNING"]) {
                self.returning();
            } else {
                self.dml_junk();
            }
        }
        if whens == 0 && self.at_end() {
            self.error_here("Expected WHEN in MERGE");
        }
        self.finish();
    }

    fn merge_clause(&mut self) {
        self.start(Kind::MergeClause);
        self.bump(); // WHEN
        self.bump_if_kw("NOT");
        if !self.bump_if_kw("MATCHED") && self.at_end() {
            self.error_here("Expected MATCHED after WHEN");
        }
        if self.kw(0, "BY") && self.kw_any(1, &["SOURCE", "TARGET"]) {
            self.bump();
            self.bump();
        }
        if self.kw(0, "AND") {
            self.bump();
            self.expect_expr("AND");
        }
        if !self.kw(0, "THEN") && self.at_end() {
            self.error_here("Expected THEN in the WHEN clause");
        }
        if self.kw(0, "THEN") {
            self.bump();
            if self.at_end() {
                self.error_here("Expected UPDATE, DELETE or INSERT after THEN");
            }
            if self.kw(0, "UPDATE") {
                self.bump();
                if self.kw(0, "SET") {
                    self.set_clause();
                }
            } else if self.kw(0, "DELETE") {
                self.bump();
            } else if self.kw(0, "INSERT") {
                self.bump();
                if self.punct(0, "(") {
                    self.column_list();
                }
                if self.kw_any(0, &["VALUES", "VALUE"]) {
                    self.values_body();
                } else if self.kw(0, "DEFAULT") && self.kw(1, "VALUES") {
                    self.bump();
                    self.bump();
                } else if self.op(0, "*") {
                    self.bump();
                }
            } else if self.kw(0, "DO") {
                self.bump();
                self.bump_if_kw("NOTHING");
            }
        }
        self.finish();
    }

    // ---- DDL

    /// CREATE / ALTER / DROP / TRUNCATE / RENAME. The object kind and the names are recorded as `Name`
    /// nodes; column definitions of CREATE TABLE as `ColumnDef`s; a query after `AS` is parsed.
    pub(super) fn ddl(&mut self) {
        self.start(Kind::Ddl);
        let first = self.text(&self.peek().expect("ddl word")).to_ascii_uppercase();
        self.bump();
        let mut object = String::new();
        let mut guard = 0;
        // Modifiers up to the object word: OR REPLACE, TEMP, UNIQUE, MATERIALIZED, ...
        if first != "TRUNCATE" || self.kw(0, "TABLE") {
            while guard < 10 {
                guard += 1;
                let Some(t) = self.peek() else { break };
                if t.kind != TokenKind::Word {
                    break;
                }
                let w = self.text(&t).to_ascii_uppercase();
                self.bump();
                if is_object_word(&w) {
                    object = w;
                    break;
                }
            }
        }
        let routine =
            matches!(object.as_str(), "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT" | "MACRO" | "PACKAGE");
        // IF [NOT] EXISTS, CONCURRENTLY, ONLY
        loop {
            if self.kw(0, "IF") && (self.kw(1, "EXISTS") || (self.kw(1, "NOT") && self.kw(2, "EXISTS"))) {
                self.bump();
                self.bump_if_kw("NOT");
                self.bump();
            } else if self.kw_any(0, &["CONCURRENTLY", "ONLY"]) && self.at_name_start_n(1) {
                self.bump();
            } else {
                break;
            }
        }
        // Names. DROP and TRUNCATE take lists.
        let list = first == "DROP" || first == "TRUNCATE" || first == "RENAME";
        let mut named = false;
        if self.at_name_start() && !(object == "INDEX" && self.kw(0, "ON")) {
            named = true;
            loop {
                self.table_name();
                if list && self.punct(0, ",") {
                    self.bump();
                    if !self.at_name_start() {
                        if self.rejects_trailing_comma() {
                            self.error_here("Expected a name after the comma");
                        }
                        break;
                    }
                    continue;
                }
                break;
            }
        } else if list && self.punct(0, ",") {
            self.error_here("Unexpected comma: a name is missing");
        }
        if self.at_end() && !object.is_empty() && !named {
            // `CREATE TABLE`, `DROP VIEW`, `ALTER INDEX` with no name.
            if matches!(
                object.as_str(),
                "TABLE" | "VIEW" | "INDEX" | "SEQUENCE" | "SCHEMA" | "DATABASE" | "FUNCTION" | "TRIGGER"
            ) {
                self.error_here(format!("Expected a name after {first} {object}"));
            }
        } else if self.at_end()
            && named
            && first == "CREATE"
            && matches!(object.as_str(), "TABLE" | "VIEW")
            && matches!(self.fam, Family::Postgres | Family::Sqlite | Family::DuckDb)
        {
            // MariaDB accepts a bare `CREATE TABLE t`; the other three need columns or a query.
            self.error_here(format!("CREATE {object} needs a definition"));
        } else if self.at_end()
            && named
            && first == "ALTER"
            && object == "TABLE"
            && matches!(self.fam, Family::Postgres | Family::Sqlite | Family::DuckDb)
        {
            self.error_here("ALTER TABLE needs an action");
        } else if first == "ALTER" && self.at_end() && !named && object.is_empty() {
            self.error_here("Expected what to ALTER");
        }
        if first == "ALTER" && self.dangling_ddl_word() {
            self.error_here("The statement ends where an operand is missing");
        }
        match object.as_str() {
            "TABLE" if first == "CREATE" && self.punct(0, "(") && !self.paren_starts_query(0) => {
                self.column_def_list();
            }
            // CREATE INDEX name ON table ... / DROP INDEX name ON table
            "INDEX" if self.kw(0, "ON") && self.at_name_start_n(1) => {
                self.bump();
                self.table_name();
            }
            _ => {}
        }
        self.loose_region(!routine);
        self.finish();
    }

    /// Does the statement (an ALTER) end on a word that always needs an operand: `ADD`, `DROP`, `RENAME TO`...?
    fn dangling_ddl_word(&self) -> bool {
        if !self.at_end() || self.pos == 0 {
            return false;
        }
        let last = self.sql[self.prev_span()].to_ascii_uppercase();
        matches!(
            last.as_str(),
            "ADD" | "DROP" | "RENAME" | "TO" | "COLUMN" | "MODIFY" | "CHANGE" | "CONSTRAINT" | "SET"
        )
    }

    fn at_name_start_n(&self, n: usize) -> bool {
        self.is_ident(n) || (self.fam == Family::SqlServer && self.op(n, "#") && self.is_ident(n + 1))
    }

    fn column_def_list(&mut self) {
        self.start(Kind::ColumnDefList);
        let open = self.next_span();
        self.bump();
        let mut state = 0; // 0 start, 1 after an item, 2 after a comma
        loop {
            if self.at_end() || self.punct(0, ")") {
                if state == 2 && !self.at_end() && self.rejects_trailing_comma() {
                    self.error(self.prev_span(), "Unexpected comma before ')'", false);
                }
                break;
            }
            if self.punct(0, ",") {
                if state != 1 {
                    let sp = self.next_span();
                    self.error(sp, "Unexpected comma: a column definition is missing", false);
                }
                state = 2;
                self.bump();
                continue;
            }
            state = 1;
            self.start(Kind::ColumnDef);
            while let Some(t) = self.peek() {
                if t.kind == TokenKind::Punct && matches!(self.text(&t), "," | ")") {
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
        self.expect_close(open);
        self.finish();
    }
}

fn is_object_word(w: &str) -> bool {
    matches!(
        w,
        "TABLE"
            | "VIEW"
            | "INDEX"
            | "SEQUENCE"
            | "SCHEMA"
            | "DATABASE"
            | "FUNCTION"
            | "PROCEDURE"
            | "TRIGGER"
            | "TYPE"
            | "EXTENSION"
            | "DOMAIN"
            | "ROLE"
            | "USER"
            | "TABLESPACE"
            | "MACRO"
            | "EVENT"
            | "PACKAGE"
            | "SERVER"
            | "SECRET"
            | "POLICY"
            | "RULE"
            | "COLLATION"
            | "STATISTICS"
            | "PUBLICATION"
            | "SUBSCRIPTION"
            | "DICTIONARY"
            | "WAREHOUSE"
    )
}
