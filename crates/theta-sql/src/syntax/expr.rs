//! Expressions: precedence climbing over operators, plus the primaries (literals, names, calls, `CASE`,
//! subqueries, casts, constructors).
//!
//! Binding powers (higher binds tighter), after PostgreSQL's table; MySQL's differences (`||` as OR, `!`) change
//! only the *shape* of a tree, never whether it parses:
//!
//! ```text
//!  1 OR   2 AND   3 NOT   4 IS/ISNULL   5 comparison   6 BETWEEN IN LIKE   7 other operators
//!  8 + -  9 * / %  10 ^  11 AT TIME ZONE, COLLATE  12 unary +/-   then postfix  ::  [ ]  .field
//! ```

use super::node::Kind;
use super::parser::{MAX_DEPTH, Parser};
use super::words;
use crate::keywords::is_type_name;
use crate::lexer::TokenKind;
use theta_core::Family;

const PREC_OR: u8 = 1;
const PREC_AND: u8 = 2;
const PREC_NOT: u8 = 3;
const PREC_IS: u8 = 4;
const PREC_CMP: u8 = 5;
const PREC_IN: u8 = 6;
const PREC_OTHER: u8 = 7;
const PREC_ADD: u8 = 8;
const PREC_MUL: u8 = 9;
const PREC_EXP: u8 = 10;
const PREC_AT: u8 = 11;
const PREC_UNARY: u8 = 12;

/// Functions whose first argument is a bare unit word: `EXTRACT(year FROM d)`, `DATEADD(day, 1, d)`.
const UNIT_FIRST: &[&str] = &[
    "EXTRACT",
    "DATEADD",
    "DATEDIFF",
    "DATEDIFF_BIG",
    "DATEPART",
    "DATENAME",
    "DATETRUNC",
    "TIMESTAMPADD",
    "TIMESTAMPDIFF",
    "DATE_PART",
    "DATE_DIFF",
    "DATE_ADD",
    "DATE_SUB",
    "DATE_TRUNC",
];

/// Functions whose arguments may be type names: `CONVERT(int, x)`, `CONVERT(x, CHAR)`.
const TYPE_ARGS: &[&str] = &["CONVERT", "TRY_CONVERT", "TRY_CAST", "SAFE_CAST", "PARSE", "TRY_PARSE"];

/// Functions with a `COLUMNS (...)` definition list inside the call.
const COLUMNS_ARG: &[&str] = &["JSON_TABLE", "XMLTABLE", "OPENJSON", "OPENXML"];

fn in_list(list: &[&str], w: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(w))
}

impl Parser<'_> {
    /// Can an expression begin at the cursor?
    pub(super) fn expr_can_start(&self) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::Number
            | TokenKind::String
            | TokenKind::DollarString
            | TokenKind::Param
            | TokenKind::QuotedIdent => true,
            TokenKind::Word => {
                let w = self.text(&t);
                !words::cannot_start_expr(self.fam, w)
                    || (self.punct(1, "(") && words::is_function_name_ok(w))
            }
            TokenKind::Punct => matches!(self.text(&t), "(" | "["),
            TokenKind::Operator => {
                let s = self.text(&t);
                s == "{" || self.is_prefix_operator(s)
            }
            _ => false,
        }
    }

    /// Is the next token an operator that is certainly never a prefix (`>`, `=`, `*`, `/`, `<=`, ...)?
    fn op_cannot_be_prefix(&self) -> bool {
        self.tok(0).is_some_and(|t| {
            t.kind == TokenKind::Operator
                && matches!(
                    self.text(&t),
                    "=" | "==" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "*" | "/" | "%" | "||" | "&&"
                )
        })
    }

    fn is_prefix_operator(&self, s: &str) -> bool {
        // `*` is only a star where a select list or argument list expects one, and `::` / `:` are never prefixes.
        s.chars().next().is_some_and(|c| "+-~!@#|?&".contains(c)) && s != "::"
    }

    /// Parse an expression. Returns false, consuming nothing, when none starts here.
    pub(super) fn expr(&mut self) -> bool {
        self.expr_bp(0)
    }

    /// An expression that the grammar requires after `what`; reports its absence when that is certain.
    pub(super) fn expect_expr(&mut self, what: &str) -> bool {
        if self.expr() {
            return true;
        }
        if self.at_terminator() {
            self.error_here(format!("Expected an expression after {what}"));
        }
        false
    }

    fn expr_bp(&mut self, min: u8) -> bool {
        if self.depth >= MAX_DEPTH {
            return false;
        }
        self.depth += 1;
        let m = self.mark();
        let ok = self.unary();
        if ok {
            while self.infix_step(m, min) {}
        }
        self.depth -= 1;
        ok
    }

    fn unary(&mut self) -> bool {
        let Some(t) = self.peek() else { return false };
        if self.kw(0, "NOT") {
            let m = self.mark();
            self.bump();
            if !self.expr_bp(PREC_NOT) && self.at_terminator() {
                self.error_here("Expected an expression after NOT");
            }
            self.wrap(m, Kind::Prefix);
            return true;
        }
        if t.kind == TokenKind::Operator && self.is_prefix_operator(self.text(&t)) {
            let m = self.mark();
            self.bump();
            if !self.expr_bp(PREC_UNARY) && self.at_terminator() {
                let op = &self.sql[self.prev_span()];
                if words::is_certain_binary_operator(op, self.fam == Family::DuckDb) {
                    self.error_here(format!("Expected an expression after '{op}'"));
                }
            }
            self.wrap(m, Kind::Prefix);
            return true;
        }
        if !self.expr_can_start() {
            return false;
        }
        let m = self.mark();
        if !self.primary() {
            return false;
        }
        self.postfix_loop(m);
        true
    }

    fn postfix_loop(&mut self, m: usize) {
        loop {
            if self.op(0, "::") {
                self.bump();
                if !self.type_name() && self.at_terminator() {
                    self.error_here("Expected a type name after ::");
                }
                self.wrap(m, Kind::Cast);
            } else if self.punct(0, "[")
                && matches!(self.fam, Family::Postgres | Family::DuckDb | Family::ClickHouse)
            {
                self.bump();
                loop {
                    if self.at_end() || self.punct(0, "]") {
                        break;
                    }
                    // `a[1:2]`: the colon is a separator; anything unparsable is kept
                    if self.punct(0, ",") || self.op(0, ":") || !self.expr() {
                        self.bump();
                    }
                }
                self.bump_if_punct("]");
                self.wrap(m, Kind::Index);
            } else if self.punct(0, ".")
                && matches!(
                    self.last_node_kind(),
                    Some(
                        Kind::Paren
                            | Kind::Subquery
                            | Kind::FuncCall
                            | Kind::Index
                            | Kind::FieldAccess
                            | Kind::Cast
                            | Kind::RowCtor
                            | Kind::ArrayCtor
                    )
                )
                && (self.is_ident(1)
                    || self.op(1, "*")
                    || self.tok(1).is_some_and(|t| t.kind == TokenKind::Number))
            {
                self.bump();
                self.bump();
                self.wrap(m, Kind::FieldAccess);
            } else {
                break;
            }
        }
    }

    /// One infix operator at binding power `>= min`, with its right operand. Returns false when the next
    /// token is no infix operator or binds too loosely.
    fn infix_step(&mut self, m: usize, min: u8) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::Word => self.infix_word(m, min),
            TokenKind::Operator => {
                let op = self.text(&t);
                let prec = match op {
                    "::" | "{" | "}" | "\\" => return false,
                    "=" | "==" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "<=>" | "=>" => PREC_CMP,
                    ":=" => PREC_OR,
                    "+" | "-" => PREC_ADD,
                    "*" | "/" | "%" | "//" => PREC_MUL,
                    "^" => PREC_EXP,
                    _ => PREC_OTHER,
                };
                if prec < min {
                    return false;
                }
                let certain = words::is_certain_binary_operator(op, self.fam == Family::DuckDb);
                let op = op.to_string();
                self.bump();
                if !self.expr_bp(prec + 1) {
                    if certain && self.at_terminator() {
                        self.error_here(format!("Expected an expression after '{op}'"));
                    } else if certain && self.fam != Family::Postgres && self.op_cannot_be_prefix() {
                        // `a > > b`, `a = = b`: no such operator can stand in front of an operand. (PostgreSQL
                        // lets users define prefix operators with any name, so it is left out.)
                        self.error_here(format!("Unexpected operator after '{op}'"));
                    }
                }
                self.wrap_binary(m, prec);
                true
            }
            _ => false,
        }
    }

    /// Wrap into a `Binary`, extending a left operand that is a chain of the same precedence.
    fn wrap_binary(&mut self, m: usize, prec: u8) {
        let sql = self.sql;
        self.wrap_merging(m, Kind::Binary, &|n| chain_prec(sql, n) == Some(prec));
    }

    fn infix_word(&mut self, m: usize, min: u8) -> bool {
        let Some(t) = self.peek() else { return false };
        let w = self.text(&t).to_ascii_uppercase();
        // (precedence, number of leading tokens that make up the operator)
        let negated = w == "NOT"
            && self.tok(1).is_some_and(|n| {
                n.kind == TokenKind::Word
                    && [
                        "IN", "LIKE", "ILIKE", "BETWEEN", "SIMILAR", "REGEXP", "RLIKE", "GLOB", "MATCH",
                        "MEMBER",
                    ]
                    .iter()
                    .any(|k| self.text(&n).eq_ignore_ascii_case(k))
            });
        let op = if negated { self.text(&self.tok(1).unwrap()).to_ascii_uppercase() } else { w.clone() };
        // PostgreSQL and DuckDB accept nearly every keyword as a bare column label, so in a select list
        // `SELECT 1 AND`, `SELECT 1 IS` and friends are an operand with an alias, not a missing operand.
        if self.fam == Family::Postgres
            && self.in_select_item(self.depth)
            && !words::pg_not_bare_label(&w)
            && self.is_terminator_at(if negated { 2 } else { 1 })
        {
            return false;
        }
        let prec = match op.as_str() {
            "OR" | "XOR" if !negated => PREC_OR,
            "AND" if !negated => PREC_AND,
            "IS" | "ISNULL" | "NOTNULL" if !negated => PREC_IS,
            "NOT" if self.kw(1, "NULL") => PREC_IS,
            "IN" | "LIKE" | "ILIKE" | "BETWEEN" | "SIMILAR" | "REGEXP" | "RLIKE" | "GLOB" | "OVERLAPS" => {
                PREC_IN
            }
            "MATCH" | "MEMBER" if self.is_infix_match() => PREC_IN,
            "SOUNDS" if self.kw(1, "LIKE") => PREC_IN,
            "DIV" | "MOD" => PREC_MUL,
            "COLLATE" | "AT" if !negated => PREC_AT,
            "AGAINST" | "OPERATOR" => PREC_OTHER,
            _ => return false,
        };
        if op == "AT" && !self.kw(1, "TIME") && !self.kw(1, "LOCAL") {
            return false;
        }
        if prec < min {
            return false;
        }
        match op.as_str() {
            "OR" | "XOR" | "AND" => {
                self.bump();
                if !self.expr_bp(prec + 1) && self.at_terminator() {
                    self.error_here(format!("Expected an expression after {op}"));
                }
                self.wrap_binary(m, prec);
            }
            "IS" => {
                self.bump();
                self.bump_if_kw("NOT");
                self.is_tail();
                self.wrap(m, Kind::Binary);
            }
            "ISNULL" | "NOTNULL" => {
                self.bump();
                self.wrap(m, Kind::Postfix);
            }
            "NOT" => {
                // `x NOT NULL` (SQLite)
                self.bump();
                self.bump();
                self.wrap(m, Kind::Postfix);
            }
            "IN" => {
                if negated {
                    self.bump();
                }
                self.bump();
                self.in_operand();
                self.wrap(m, Kind::Binary);
            }
            "BETWEEN" => {
                if negated {
                    self.bump();
                }
                self.bump();
                if self.kw_any(0, &["SYMMETRIC", "ASYMMETRIC"]) {
                    self.bump();
                }
                let lo = self.expr_bp(PREC_OTHER);
                if self.kw(0, "AND") {
                    self.bump();
                    if !self.expr_bp(PREC_OTHER) && self.at_terminator() {
                        self.error_here("Expected an expression after AND");
                    }
                } else if lo && self.at_terminator() {
                    self.error_here("BETWEEN needs AND and a second bound");
                }
                self.wrap(m, Kind::Binary);
            }
            "LIKE" | "ILIKE" | "SIMILAR" | "REGEXP" | "RLIKE" | "GLOB" | "MATCH" | "SOUNDS" | "MEMBER" => {
                if negated {
                    self.bump();
                }
                self.bump();
                if matches!(op.as_str(), "SIMILAR" | "SOUNDS" | "MEMBER") {
                    self.bump(); // TO / LIKE / OF
                }
                if !self.expr_bp(PREC_OTHER) && self.at_terminator() {
                    self.error_here(format!("Expected an expression after {op}"));
                }
                if self.kw(0, "ESCAPE") {
                    self.bump();
                    if !self.expr_bp(PREC_OTHER) && self.at_terminator() {
                        self.error_here("Expected an expression after ESCAPE");
                    }
                }
                self.wrap(m, Kind::Binary);
            }
            "OVERLAPS" | "AGAINST" | "DIV" | "MOD" => {
                self.bump();
                if !self.expr_bp(prec + 1) && self.at_terminator() {
                    self.error_here(format!("Expected an expression after {op}"));
                }
                self.wrap(m, Kind::Binary);
            }
            "OPERATOR" => {
                self.bump();
                if self.punct(0, "(") {
                    self.skip_group();
                }
                self.expr_bp(prec + 1);
                self.wrap(m, Kind::Binary);
            }
            "COLLATE" => {
                self.bump();
                if self.tok(0).is_some_and(|t| {
                    matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String)
                }) {
                    self.bump();
                    while self.punct(0, ".") && self.is_ident(1) {
                        self.bump();
                        self.bump();
                    }
                }
                self.wrap(m, Kind::Postfix);
            }
            "AT" => {
                self.bump();
                if self.kw(0, "LOCAL") {
                    self.bump();
                } else {
                    self.bump();
                    self.bump_if_kw("ZONE");
                    self.expr_bp(PREC_UNARY);
                }
                self.wrap(m, Kind::Postfix);
            }
            _ => return false,
        }
        true
    }

    /// `MATCH` is an infix operator in SQLite; in MySQL it begins `MATCH (cols) AGAINST (...)`, which is a
    /// primary and never reaches this point after an operand.
    fn is_infix_match(&self) -> bool {
        true
    }

    /// What follows `IS [NOT]`.
    fn is_tail(&mut self) {
        if self.kw_any(0, &["NULL", "TRUE", "FALSE", "UNKNOWN", "DOCUMENT", "NORMALIZED"]) {
            self.bump();
        } else if self.kw(0, "DISTINCT") && !self.kw(1, "FROM") && self.is_terminator_at(1) {
            self.bump();
            self.error_here("Expected FROM after IS DISTINCT");
        } else if self.kw(0, "DISTINCT") && self.kw(1, "FROM") {
            self.bump();
            self.bump();
            if !self.expr_bp(PREC_OTHER) && self.at_terminator() {
                self.error_here("Expected an expression after IS DISTINCT FROM");
            }
        } else if self.kw(0, "JSON") {
            self.bump();
            while self.kw_any(0, &["VALUE", "ARRAY", "OBJECT", "SCALAR", "WITH", "WITHOUT", "UNIQUE", "KEYS"])
            {
                self.bump();
            }
        } else if self.kw(0, "OF") && self.punct(1, "(") {
            self.bump();
            self.skip_group();
        } else if self.kw_any(0, &["NFC", "NFD", "NFKC", "NFKD"]) {
            self.bump();
            self.bump_if_kw("NORMALIZED");
        } else if !self.expr_bp(PREC_CMP + 1) && self.at_terminator() {
            // `IS` with nothing after it is incomplete in every dialect (SQLite's `a IS b` needs b).
            self.error_here("Expected NULL, TRUE, FALSE or DISTINCT FROM after IS");
        }
    }

    fn in_operand(&mut self) {
        if self.punct(0, "(") {
            if self.paren_starts_query(0) {
                self.subquery();
            } else {
                self.start(Kind::RowCtor);
                let open = self.next_span();
                self.bump();
                if self.punct(0, ")")
                    && matches!(self.fam, Family::Postgres | Family::MySql | Family::SqlServer)
                {
                    // `x IN ()`: PostgreSQL, MySQL and T-SQL reject an empty list (SQLite accepts it).
                    let close = self.next_span();
                    self.error(open.start..close.end, "IN needs at least one value", false);
                }
                self.expr_list_until_close();
                self.expect_close(open);
                self.finish();
            }
        } else if !self.expr_bp(PREC_OTHER) && self.at_terminator() {
            self.error_here("Expected a list or subquery after IN");
        }
    }

    // ---- primaries

    fn primary(&mut self) -> bool {
        let Some(t) = self.peek() else { return false };
        match t.kind {
            TokenKind::Number | TokenKind::DollarString => {
                self.start(Kind::Literal);
                self.bump();
                self.finish();
                true
            }
            TokenKind::String => {
                self.start(Kind::Literal);
                self.bump();
                // `'a' 'b'` concatenates in standard SQL and MySQL.
                while self.tok(0).is_some_and(|n| n.kind == TokenKind::String) {
                    self.bump();
                }
                self.finish();
                true
            }
            TokenKind::Param => {
                self.start(Kind::Param);
                self.bump();
                self.finish();
                true
            }
            TokenKind::Punct => match self.text(&t) {
                "(" => self.paren(),
                "[" => {
                    self.array_ctor(false);
                    true
                }
                _ => false,
            },
            TokenKind::Operator if self.text(&t) == "{" => {
                self.brace_literal();
                true
            }
            TokenKind::Word | TokenKind::QuotedIdent => self.primary_word(),
            _ => false,
        }
    }

    /// The operand of `TOP`: a number, a variable or a parenthesised expression, but not an arithmetic
    /// expression (`TOP (10) * FROM t` is a star, not a product).
    pub(super) fn top_operand(&mut self) {
        if self.punct(0, "(") {
            self.paren();
        } else if self.tok(0).is_some_and(|t| matches!(t.kind, TokenKind::Number | TokenKind::Param)) {
            self.bump();
        }
    }

    fn paren(&mut self) -> bool {
        if self.paren_starts_query(0) {
            self.subquery();
            return true;
        }
        let open = self.next_span();
        let m = self.mark();
        self.bump();
        if self.punct(0, ")") {
            self.bump();
            self.wrap(m, Kind::RowCtor);
            return true;
        }
        let mut commas = 0;
        let mut state = 0; // 0 start, 1 after an item, 2 after a comma
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
                commas += 1;
                state = 2;
                self.bump();
                continue;
            }
            state = 1;
            if self.expr() {
                continue;
            }
            if self.kw_any(0, &["AND", "OR"]) {
                // `(and x)`: a binary-only operator with nothing before it.
                self.error_here("Expected an expression before AND/OR");
            }
            // Anything else this grammar does not model: keep it, do not complain.
            self.start(Kind::Unknown);
            self.skip_to_close_or_comma();
            self.finish();
        }
        self.expect_close(open);
        self.wrap(m, if commas > 0 { Kind::RowCtor } else { Kind::Paren });
        true
    }

    fn skip_to_close_or_comma(&mut self) {
        let mut first = true;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    ")" | "," if !first => return,
                    ")" => return,
                    "(" => {
                        self.skip_group();
                        first = false;
                        continue;
                    }
                    _ => {}
                }
            }
            first = false;
            self.bump();
        }
    }

    fn array_ctor(&mut self, has_keyword: bool) {
        let m = self.mark();
        if has_keyword {
            self.bump();
        }
        let open = self.next_span();
        self.bump(); // [
        loop {
            if self.at_end() || self.punct(0, "]") {
                break;
            }
            if self.punct(0, ",") {
                self.bump();
                continue;
            }
            if self.expr() {
                continue;
            }
            self.start(Kind::Unknown);
            self.bump();
            self.finish();
        }
        if self.punct(0, "]") {
            self.bump();
        } else if self.at_end() {
            // Every dialect that has array literals closes them.
            self.error(open, "Unclosed '['", true);
        }
        self.wrap(m, Kind::ArrayCtor);
    }

    /// `{'a': 1}` (DuckDB, ClickHouse), `{d '2020-01-01'}` (ODBC escape in MySQL and T-SQL).
    fn brace_literal(&mut self) {
        let m = self.mark();
        let open = self.next_span();
        self.bump();
        loop {
            if self.at_end() || self.op(0, "}") || self.punct(0, ")") {
                break;
            }
            if self.punct(0, ",") {
                self.bump();
                continue;
            }
            if self.is_word_tok(0)
                && self.tok(1).is_some_and(|t| t.kind == TokenKind::String)
                && self.fam != Family::DuckDb
            {
                // ODBC `{d '..'}`, `{ts '..'}`, `{fn ..}`
                self.bump();
                continue;
            }
            if self.expr() {
                continue;
            }
            self.start(Kind::Unknown);
            self.bump();
            self.finish();
        }
        if self.op(0, "}") {
            self.bump();
        } else if self.at_end() {
            self.error(open, "Unclosed '{'", true);
        }
        self.wrap(m, Kind::ArrayCtor);
    }

    fn primary_word(&mut self) -> bool {
        let Some(t) = self.peek() else { return false };
        if t.kind == TokenKind::Word {
            let text = self.text(&t);
            let up = text.to_ascii_uppercase();
            let next_paren = self.punct(1, "(");
            match up.as_str() {
                "NULL" | "TRUE" | "FALSE" if !self.punct(1, ".") => {
                    self.start(Kind::Literal);
                    self.bump();
                    self.finish();
                    return true;
                }
                "CASE" => {
                    self.case_expr();
                    return true;
                }
                "EXISTS" if next_paren => {
                    let m = self.mark();
                    self.bump();
                    if self.paren_starts_query(0) {
                        self.subquery();
                    } else {
                        self.arg_list_for("");
                    }
                    self.wrap(m, Kind::Exists);
                    return true;
                }
                "EXISTS" if self.is_query_start(1) => {
                    // `EXISTS SELECT ...`: the subquery needs its parentheses.
                    let span = self.next_span();
                    self.error(span, "EXISTS needs a parenthesised subquery", false);
                    self.start(Kind::Unknown);
                    self.bump();
                    self.finish();
                    return true;
                }
                "ARRAY" if self.punct(1, "[") => {
                    self.array_ctor(true);
                    return true;
                }
                "ROW" if next_paren => {
                    let m = self.mark();
                    self.bump();
                    self.row_ctor();
                    self.wrap(m, Kind::RowCtor);
                    return true;
                }
                "INTERVAL" if self.interval_follows() => {
                    self.interval();
                    return true;
                }
                "NEXT" if self.kw(1, "VALUE") && self.kw(2, "FOR") => {
                    let m = self.mark();
                    self.bump();
                    self.bump();
                    self.bump();
                    self.table_name();
                    self.wrap(m, Kind::FuncCall);
                    return true;
                }
                "DEFAULT" => {
                    self.start(Kind::Literal);
                    self.bump();
                    self.finish();
                    return true;
                }
                _ if words::is_typed_literal_word(&up)
                    && self.tok(1).is_some_and(|n| n.kind == TokenKind::String) =>
                {
                    self.start(Kind::Literal);
                    self.bump();
                    self.bump();
                    self.finish();
                    return true;
                }
                _ => {}
            }
            if words::cannot_start_expr(self.fam, text) && !(next_paren && words::is_function_name_ok(text)) {
                return false;
            }
        }
        self.name_or_call()
    }

    fn name_or_call(&mut self) -> bool {
        let m = self.mark();
        self.start(Kind::Name);
        let star = self.name_tokens(true);
        self.finish();
        if !star && self.punct(0, "(") {
            let fname = self.last_name_last_part();
            self.arg_list_for(&fname);
            self.call_tail();
            self.wrap(m, Kind::FuncCall);
        } else if star {
            self.retag_last(Kind::Star);
        } else {
            self.retag_last(Kind::ColumnRef);
        }
        true
    }

    /// The last identifier of the `Name` node just built, as text.
    fn last_name_last_part(&mut self) -> String {
        match self.last_node() {
            Some(n) => n
                .ident_tokens()
                .last()
                .map(|t| crate::lexer::unquote_ident(&self.sql[t.span.clone()]))
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    /// `FILTER (...)`, `WITHIN GROUP (...)`, `OVER ...`, `IGNORE NULLS` after a call's argument list.
    fn call_tail(&mut self) {
        loop {
            if self.kw(0, "FILTER") && self.punct(1, "(") {
                self.start(Kind::Filter);
                self.bump();
                let open = self.next_span();
                self.bump();
                self.bump_if_kw("WHERE");
                self.expr();
                self.expect_close(open);
                self.finish();
            } else if self.kw(0, "WITHIN") && self.kw(1, "GROUP") && self.punct(2, "(") {
                self.start(Kind::WithinGroup);
                self.bump();
                self.bump();
                let open = self.next_span();
                self.bump();
                if self.kw(0, "ORDER") {
                    self.order_by();
                }
                self.expect_close(open);
                self.finish();
            } else if self.kw(0, "OVER") && (self.punct(1, "(") || self.is_ident(1)) {
                self.start(Kind::Over);
                self.bump();
                if self.punct(0, "(") {
                    self.window_def();
                } else {
                    self.bump();
                }
                self.finish();
            } else if self.kw_any(0, &["IGNORE", "RESPECT"]) && self.kw(1, "NULLS") {
                self.bump();
                self.bump();
            } else {
                break;
            }
        }
    }

    /// `( [base] [PARTITION BY ..] [ORDER BY ..] [frame] )`
    pub(super) fn window_def(&mut self) {
        self.start(Kind::WindowDef);
        let open = self.next_span();
        self.bump();
        if self.is_ident(0) && !self.kw_any(0, &["PARTITION", "ORDER", "ROWS", "RANGE", "GROUPS"]) {
            self.bump();
        }
        loop {
            if self.at_end() || self.punct(0, ")") {
                break;
            }
            if self.kw(0, "PARTITION") && self.kw(1, "BY") {
                self.bump();
                self.bump();
                loop {
                    if !self.expr() {
                        break;
                    }
                    if !self.bump_if_punct(",") {
                        break;
                    }
                }
            } else if self.kw(0, "ORDER") && self.kw(1, "BY") {
                self.order_by();
            } else {
                self.loose_items("");
                break;
            }
        }
        self.expect_close(open);
        self.finish();
    }

    /// `( args )`. `fname` is the function's last name part, for the few calls whose arguments are not all
    /// expressions.
    pub(super) fn arg_list_for(&mut self, fname: &str) {
        self.start(Kind::ArgList);
        let open = self.next_span();
        self.bump();
        if self.kw_any(0, &["SELECT", "WITH", "VALUES"]) || (self.kw(0, "TABLE") && self.is_query_start(0)) {
            self.query();
        }
        self.loose_items(fname);
        self.expect_close(open);
        self.finish();
    }

    /// The body of an argument list or window frame: expressions, with the syntax words of calls
    /// (`FROM`, `FOR`, `AS type`, `ORDER BY`, `SEPARATOR`, ...) kept as plain tokens. A word right after an
    /// operand without a comma is never an operand, so `trim(both 'x' from y)` and `x NULLS FIRST` do not
    /// invent identifiers.
    fn loose_items(&mut self, fname: &str) {
        let mut last_expr = false;
        let mut arg_index = 0;
        let mut seen_as = false;
        let mut seen_anything = false;
        let mut item_started = false;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    ")" => break,
                    "," => {
                        if !item_started {
                            // `f(,1)` and `f(1,,2)`: an argument is missing.
                            self.error(t.start..t.end, "Unexpected comma: an argument is missing", false);
                        }
                        item_started = false;
                        self.bump();
                        last_expr = false;
                        arg_index += 1;
                        continue;
                    }
                    _ => {}
                }
            }
            if t.kind == TokenKind::Operator && self.text(&t) == "*" && !last_expr {
                item_started = true;
                self.start(Kind::Star);
                self.bump();
                self.finish();
                last_expr = true;
                continue;
            }
            if matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent)
                && (self.op(1, "=>") || self.op(1, ":="))
            {
                // Named argument: the name is not a column.
                self.bump();
                self.bump();
                last_expr = false;
                continue;
            }
            seen_anything = true;
            item_started = true;
            if t.kind == TokenKind::Word {
                let w = self.text(&t).to_string();
                if w.eq_ignore_ascii_case("AS") {
                    seen_as = true;
                    self.bump();
                    if self.is_ident(0) {
                        self.type_name();
                    }
                    last_expr = true;
                    continue;
                }
                let call_or_dot = self.punct(1, "(") || self.punct(1, ".");
                if in_list(COLUMNS_ARG, fname) && w.eq_ignore_ascii_case("COLUMNS") && self.punct(1, "(") {
                    self.start(Kind::Unknown);
                    self.bump();
                    self.skip_group();
                    self.finish();
                    continue;
                }
                if in_list(UNIT_FIRST, fname) && arg_index == 0 && !last_expr && !call_or_dot {
                    self.bump();
                    last_expr = true;
                    continue;
                }
                if in_list(TYPE_ARGS, fname)
                    && !call_or_dot
                    && !last_expr
                    && (is_type_name(&w) || is_sign_word(&w))
                {
                    self.type_name();
                    last_expr = true;
                    continue;
                }
                let operand_next = self.tok(1).is_some_and(|n| {
                    matches!(
                        n.kind,
                        TokenKind::Word | TokenKind::String | TokenKind::Number | TokenKind::Param
                    )
                });
                let keyword = words::is_loose_keyword(&w)
                    && (!words::is_ambiguous_loose(&w) || last_expr || operand_next);
                if !call_or_dot && (keyword || last_expr) && !self.is_typed_literal_here() {
                    self.bump();
                    last_expr = last_expr && !keyword;
                    continue;
                }
            }
            if self.expr_can_start() && self.expr() {
                last_expr = true;
                continue;
            }
            // Unmodelled token inside the call: keep it.
            self.bump();
            last_expr = false;
        }
        if !seen_as
            && seen_anything
            && self.fam != Family::ClickHouse
            && ["CAST", "TRY_CAST", "SAFE_CAST"].iter().any(|f| f.eq_ignore_ascii_case(fname))
            && self.punct(0, ")")
        {
            // `CAST(x)`: every dialect (ClickHouse excepted: `CAST(x, 'T')`) spells the target with AS.
            self.error_here("CAST needs AS and a type");
        }
    }

    fn is_typed_literal_here(&self) -> bool {
        self.tok(0).is_some_and(|t| {
            t.kind == TokenKind::Word
                && words::is_typed_literal_word(self.text(&t))
                && self.tok(1).is_some_and(|n| n.kind == TokenKind::String)
        })
    }

    fn case_expr(&mut self) {
        let m = self.mark();
        let case_span = self.next_span();
        self.bump();
        if !self.kw(0, "WHEN") {
            self.expr();
        }
        let mut whens = 0;
        while self.kw(0, "WHEN") {
            whens += 1;
            self.start(Kind::CaseWhen);
            self.bump();
            if !self.expr() && self.at_terminator() {
                self.error_here("Expected a condition after WHEN");
            }
            if self.kw(0, "THEN") {
                self.bump();
                if !self.expr() && self.at_terminator() {
                    self.error_here("Expected a result after THEN");
                }
            }
            self.finish();
        }
        if self.kw(0, "ELSE") {
            self.bump();
            if !self.expr() && self.at_terminator() {
                self.error_here("Expected a result after ELSE");
            }
        }
        if whens == 0 && self.kw(0, "END") {
            // `CASE x END`: at least one WHEN is required in every dialect.
            self.error(case_span.clone(), "CASE needs at least one WHEN", false);
        }
        if self.kw(0, "END") {
            self.bump();
        } else if self.at_end() {
            // Every dialect closes a CASE expression with END.
            self.error(case_span, "CASE is missing END", true);
        }
        self.wrap(m, Kind::Case);
    }

    fn interval_follows(&self) -> bool {
        let Some(n) = self.tok(1) else { return false };
        match n.kind {
            TokenKind::String | TokenKind::Number | TokenKind::Param => true,
            TokenKind::Punct => self.text(&n) == "(",
            TokenKind::Word | TokenKind::QuotedIdent => !words::cannot_start_expr(self.fam, self.text(&n)),
            TokenKind::Operator => matches!(self.text(&n), "-" | "+"),
            _ => false,
        }
    }

    fn interval(&mut self) {
        let m = self.mark();
        self.bump();
        if self.tok(0).is_some_and(|t| t.kind == TokenKind::String) {
            self.start(Kind::Literal);
            self.bump();
            self.finish();
        } else {
            self.expr_bp(PREC_UNARY);
        }
        while self.tok(0).is_some_and(|t| {
            t.kind == TokenKind::Word && words::is_interval_unit(self.text(&t))
                || self.text(&t).eq_ignore_ascii_case("TO")
                    && self
                        .tok(1)
                        .is_some_and(|n| n.kind == TokenKind::Word && words::is_interval_unit(self.text(&n)))
        }) {
            self.bump();
            if self.punct(0, "(") {
                self.skip_group();
            }
        }
        self.wrap(m, Kind::Interval);
    }

    /// A type name: words, qualifiers, `(args)`, `[]`. Returns whether anything was consumed.
    pub(super) fn type_name(&mut self) -> bool {
        if !self.is_ident(0) || self.at_terminator() {
            return false;
        }
        self.start(Kind::TypeName);
        self.bump();
        while self.punct(0, ".") && self.is_ident(1) {
            self.bump();
            self.bump();
        }
        loop {
            if self
                .tok(0)
                .is_some_and(|t| t.kind == TokenKind::Word && words::is_type_continuation(self.text(&t)))
            {
                self.bump();
            } else if (self.kw_any(0, &["WITH", "WITHOUT"]) && self.kw(1, "TIME") && self.kw(2, "ZONE"))
                || (self.kw(0, "CHARACTER") && self.kw(1, "SET") && self.is_ident(2))
            {
                self.bump();
                self.bump();
                self.bump();
            } else if self.kw_any(0, &["CHARSET", "CHAR"]) && self.is_ident(1) && self.kw(0, "CHARSET") {
                self.bump();
                self.bump();
            } else if self.punct(0, "(") {
                self.skip_group();
            } else if self.punct(0, "[") {
                self.bump();
                while !self.at_end() && !self.punct(0, "]") {
                    self.bump();
                }
                self.bump_if_punct("]");
            } else {
                break;
            }
        }
        self.finish();
        true
    }
}

/// The precedence of the operator of a flat `Binary` chain (looking at its first operator token), for the
/// operators that chain: AND, OR and the arithmetic / string operators.
fn chain_prec(sql: &str, n: &super::node::Node) -> Option<u8> {
    let op = n.tokens().next()?;
    let text = op.text(sql);
    Some(match text.to_ascii_uppercase().as_str() {
        "OR" | "XOR" => PREC_OR,
        "AND" => PREC_AND,
        "+" | "-" => PREC_ADD,
        "*" | "/" | "%" | "//" | "DIV" | "MOD" => PREC_MUL,
        "^" => PREC_EXP,
        "=" | "==" | "<" | ">" | "<=" | ">=" | "<>" | "!=" | "<=>" | "=>" => PREC_CMP,
        t if op.kind == Kind::Token(TokenKind::Operator) && !matches!(t, "::" | ":=") => PREC_OTHER,
        _ => return None,
    })
}

fn is_sign_word(w: &str) -> bool {
    [
        "SIGNED", "UNSIGNED", "BINARY", "CHAR", "NCHAR", "JSON", "DATE", "DATETIME", "TIME", "DECIMAL",
        "INTEGER",
    ]
    .iter()
    .any(|k| k.eq_ignore_ascii_case(w))
}
