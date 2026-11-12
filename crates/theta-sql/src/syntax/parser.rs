//! Parser core: the tree builder, token cursor, statement dispatch and the shared recovery helpers.
//!
//! # How the tree is built
//!
//! Parsing runs over the *significant* tokens (no whitespace, no comments). Trivia is put back when a token
//! is consumed: `bump` first places every trivia token that precedes it. A node therefore starts at its first
//! significant token and ends at its last, and trivia between siblings belongs to the parent. Nodes are built
//! on a stack of frames (`start` / `finish`) so a grammar function reads like the grammar; a left operand that
//! was parsed before we knew it would become a `Binary` is wrapped afterwards with `mark` / `wrap`.
//!
//! # When is a syntax error reported?
//!
//! Only when the text cannot be valid in the dialect (`error` is called at exactly these sites, each with a
//! comment saying why it is certain):
//!
//! * a `(` with no `)` before the statement ends, and a `)` with no `(`;
//! * an unterminated string, quoted identifier, block comment or dollar quote;
//! * a binary operator, `AND`, `OR`, `NOT`, `IS`, `BETWEEN .. AND`, `IN`, `LIKE` with no right operand;
//! * `FROM`, `JOIN`, `INTO`, `UPDATE`, `DELETE FROM` followed by nothing that can name a table;
//! * a clause that needs an expression (`WHERE`, `HAVING`, `ON`, `GROUP BY`, `ORDER BY`, `CASE WHEN`) followed by none;
//! * a select list that is empty or ends in a comma, in the dialects that reject it (PostgreSQL and DuckDB
//!   allow `SELECT FROM t`; DuckDB allows a trailing comma);
//! * `CASE` with no `END`, `UPDATE` with no `SET`, a set operator with no right side.
//!
//! Everything else the grammar cannot place goes into an `Unknown` node and is silent.

use super::node::{Kind, Node, ParseError};
use super::words;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::script::segments;
use std::ops::Range;
use theta_core::{DatabaseKind, Family};

/// The result of parsing: a lossless tree plus the errors the parser is certain about.
#[derive(Debug, Clone)]
pub struct Parse {
    pub root: Node,
    pub errors: Vec<ParseError>,
    pub kind: DatabaseKind,
}

impl Parse {
    /// The statement nodes of the script, in order (empty `;` statements excluded).
    pub fn statements(&self) -> impl Iterator<Item = &Node> {
        self.root.children_of(Kind::Statement)
    }
}

/// Parse `sql` as a script in the dialect of `kind`. Never fails; never panics.
pub fn parse(sql: &str, kind: DatabaseKind) -> Parse {
    let raw = tokenize(sql, kind);
    let segs = segments(sql, &raw, kind);
    let sig: Vec<usize> = raw.iter().enumerate().filter(|(_, t)| !t.is_trivia()).map(|(i, _)| i).collect();
    let mut p = Parser {
        sql,
        fam: kind.family(),
        kind,
        raw,
        sig,
        pos: 0,
        end: 0,
        raw_pos: 0,
        frames: vec![Frame { kind: Kind::Root, children: Vec::new() }],
        errors: Vec::new(),
        depth: 0,
        sel_item_depth: None,
    };
    p.errors_for_unterminated();
    for seg in &segs {
        let seg_end = p.sig.partition_point(|&r| r < seg.tokens.end);
        // Tokens between segments that belong to no statement: a T-SQL `GO` line.
        let seg_start = p.sig.partition_point(|&r| r < seg.tokens.start);
        if p.pos < seg_start {
            p.end = seg_start;
            p.start(Kind::BatchSeparator);
            while p.pos < seg_start {
                p.bump();
            }
            p.finish();
        }
        if p.pos >= seg_end && !seg.has_semi {
            continue;
        }
        p.end = seg_end;
        p.start(Kind::Statement);
        if p.pos < p.end {
            p.statement();
        }
        // Whatever the statement grammar did not consume is kept, silently.
        p.unknown_rest();
        if seg.has_semi && p.pos < p.sig.len() {
            p.end = p.pos + 1;
            p.bump();
        }
        p.finish();
    }
    p.end = p.sig.len();
    p.pos = p.pos.min(p.end);
    if p.pos < p.end {
        p.start(Kind::Unknown);
        while p.pos < p.end {
            p.bump();
        }
        p.finish();
    }
    p.flush_trivia();
    let mut root_frame = p.frames.pop().expect("root frame");
    debug_assert!(p.frames.is_empty());
    let root =
        Node { kind: Kind::Root, span: 0..sql.len(), children: std::mem::take(&mut root_frame.children) };
    let mut errors = p.errors;
    // MySQL runs the body of `/*! ... */` and `/*M! ... */`: text the tree sees as a comment is code, so no
    // "missing piece" judgement about such a script is safe.
    if kind.family() == Family::MySql && (sql.contains("/*!") || sql.contains("/*M!")) {
        errors.clear();
    }
    errors.sort_by_key(|e| (e.span.start, e.span.end));
    errors.dedup();
    Parse { root, errors, kind }
}

pub(super) struct Frame {
    kind: Kind,
    children: Vec<Node>,
}

pub(super) struct Parser<'a> {
    pub(super) sql: &'a str,
    pub(super) fam: Family,
    #[allow(dead_code)]
    pub(super) kind: DatabaseKind,
    raw: Vec<Token>,
    /// Indexes into `raw` of the significant tokens.
    sig: Vec<usize>,
    /// Cursor into `sig`.
    pub(super) pos: usize,
    /// Exclusive limit into `sig`: the end of the statement being parsed.
    pub(super) end: usize,
    /// Next raw token not yet placed in the tree.
    raw_pos: usize,
    frames: Vec<Frame>,
    pub(super) errors: Vec<ParseError>,
    /// Recursion depth of expression and query parsing; past `MAX_DEPTH` nesting is skipped, not parsed.
    pub(super) depth: u32,
    /// `depth` at the select item being parsed, to tell its top-level expression from nested ones.
    pub(super) sel_item_depth: Option<u32>,
}

/// Deep enough for any hand-written SQL, shallow enough for the default thread stack (the editor parses on a
/// worker thread with 2 MB).
pub(super) const MAX_DEPTH: u32 = 96;

impl<'a> Parser<'a> {
    // ---- token cursor

    pub(super) fn tok(&self, n: usize) -> Option<Token> {
        let i = self.pos + n;
        if i < self.end { Some(self.raw[self.sig[i]]) } else { None }
    }

    pub(super) fn peek(&self) -> Option<Token> {
        self.tok(0)
    }

    pub(super) fn at_end(&self) -> bool {
        self.pos >= self.end
    }

    pub(super) fn text(&self, t: &Token) -> &'a str {
        &self.sql[t.start..t.end]
    }

    pub(super) fn kw(&self, n: usize, kw: &str) -> bool {
        self.tok(n).is_some_and(|t| words::tok_is_word(self.sql, &t, kw))
    }

    pub(super) fn kw_any(&self, n: usize, kws: &[&str]) -> bool {
        self.tok(n).is_some_and(|t| {
            t.kind == TokenKind::Word && kws.iter().any(|k| self.text(&t).eq_ignore_ascii_case(k))
        })
    }

    pub(super) fn punct(&self, n: usize, p: &str) -> bool {
        self.tok(n).is_some_and(|t| t.kind == TokenKind::Punct && self.text(&t) == p)
    }

    pub(super) fn op(&self, n: usize, o: &str) -> bool {
        self.tok(n).is_some_and(|t| t.kind == TokenKind::Operator && self.text(&t) == o)
    }

    pub(super) fn is_word_tok(&self, n: usize) -> bool {
        self.tok(n).is_some_and(|t| t.kind == TokenKind::Word)
    }

    /// Identifier-like: a bare word or a quoted identifier.
    pub(super) fn is_ident(&self, n: usize) -> bool {
        self.tok(n).is_some_and(|t| matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent))
    }

    /// Start of the next token, or the end of the statement's last token when there is none.
    pub(super) fn here(&self) -> usize {
        match self.tok(0) {
            Some(t) => t.start,
            None if self.pos > 0 => self.raw[self.sig[self.pos - 1]].end,
            None => 0,
        }
    }

    /// The span of the next token, or an empty span at the end of the statement.
    pub(super) fn next_span(&self) -> Range<usize> {
        match self.tok(0) {
            Some(t) => t.start..t.end,
            None => {
                let e = self.here();
                e..e
            }
        }
    }

    /// Span of the last significant token consumed.
    pub(super) fn prev_span(&self) -> Range<usize> {
        if self.pos == 0 {
            return 0..0;
        }
        let t = self.raw[self.sig[self.pos - 1]];
        t.start..t.end
    }

    // ---- tree building

    pub(super) fn flush_trivia(&mut self) {
        while self.raw_pos < self.raw.len() && self.raw[self.raw_pos].is_trivia() {
            let t = self.raw[self.raw_pos];
            self.frames.last_mut().expect("frame").children.push(Node {
                kind: Kind::Token(t.kind),
                span: t.start..t.end,
                children: Vec::new(),
            });
            self.raw_pos += 1;
        }
    }

    /// Consume the next significant token into the current node.
    pub(super) fn bump(&mut self) -> Option<Token> {
        if self.pos >= self.end {
            return None;
        }
        self.flush_trivia();
        let ri = self.sig[self.pos];
        debug_assert_eq!(ri, self.raw_pos);
        let t = self.raw[ri];
        self.frames.last_mut().expect("frame").children.push(Node {
            kind: Kind::Token(t.kind),
            span: t.start..t.end,
            children: Vec::new(),
        });
        self.raw_pos = ri + 1;
        self.pos += 1;
        Some(t)
    }

    pub(super) fn bump_if_kw(&mut self, kw: &str) -> bool {
        if self.kw(0, kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn bump_if_punct(&mut self, p: &str) -> bool {
        if self.punct(0, p) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn start(&mut self, kind: Kind) {
        self.flush_trivia();
        self.frames.push(Frame { kind, children: Vec::new() });
    }

    /// Like `start`, with `adopt` (an already built node, e.g. a leading `WITH`) as the first child.
    pub(super) fn start_adopting(&mut self, kind: Kind, adopt: Option<Node>) {
        match adopt {
            Some(n) => {
                // The adopted node is already behind us: the trivia that follows it belongs *inside* the new
                // node, after it, so open the frame first.
                self.frames.push(Frame { kind, children: vec![n] });
                self.flush_trivia();
            }
            None => self.start(kind),
        }
    }

    pub(super) fn finish(&mut self) {
        let f = self.frames.pop().expect("frame");
        let node = self.make(f.kind, f.children);
        self.frames.last_mut().expect("root frame").children.push(node);
    }

    pub(super) fn make(&self, kind: Kind, children: Vec<Node>) -> Node {
        let span = match (children.first(), children.last()) {
            (Some(a), Some(b)) => a.span.start..b.span.end,
            _ => {
                let at =
                    if self.raw_pos < self.raw.len() { self.raw[self.raw_pos].start } else { self.sql.len() };
                at..at
            }
        };
        Node { kind, span, children }
    }

    /// Index to pass to `wrap` later; trivia before the operand about to be parsed goes to the parent.
    pub(super) fn mark(&mut self) -> usize {
        self.flush_trivia();
        self.frames.last().expect("frame").children.len()
    }

    /// Like `wrap`, but a first child that `merge_ok` accepts and that already is a node of `kind` is
    /// extended instead of nested: `a + b + c` is one flat `Binary`, not a left-leaning tower. That keeps the
    /// tree depth proportional to real nesting (parentheses, subqueries), never to the length of a chain.
    pub(super) fn wrap_merging(&mut self, mark: usize, kind: Kind, merge_ok: &dyn Fn(&Node) -> bool) {
        let f = self.frames.last_mut().expect("frame");
        let mut kids: Vec<Node> = f.children.drain(mark..).collect();
        if kids.first().is_some_and(|n| n.kind == kind && merge_ok(n)) {
            let mut first = kids.remove(0);
            let mut merged = std::mem::take(&mut first.children);
            merged.append(&mut kids);
            kids = merged;
        }
        let node = self.make(kind, kids);
        self.frames.last_mut().expect("frame").children.push(node);
    }

    /// Wrap everything built since `mark` into a node of `kind`.
    pub(super) fn wrap(&mut self, mark: usize, kind: Kind) {
        let f = self.frames.last_mut().expect("frame");
        let kids: Vec<Node> = f.children.drain(mark..).collect();
        let node = self.make(kind, kids);
        self.frames.last_mut().expect("frame").children.push(node);
    }

    pub(super) fn last_node(&self) -> Option<&Node> {
        self.frames.last().and_then(|f| f.children.iter().rev().find(|c| !c.is_trivia()))
    }

    pub(super) fn last_node_kind(&self) -> Option<Kind> {
        self.last_node().map(|n| n.kind)
    }

    /// Change the kind of the last node built in the current frame (a `Name` that turned out to be a column).
    pub(super) fn retag_last(&mut self, kind: Kind) {
        if let Some(n) = self.frames.last_mut().and_then(|f| f.children.last_mut()) {
            n.kind = kind;
        }
    }

    /// Take the last child node of the current frame out (to adopt it elsewhere).
    pub(super) fn pop_child(&mut self) -> Option<Node> {
        self.frames.last_mut().expect("frame").children.pop()
    }

    /// Kind of the node currently being built, for context-dependent decisions.
    #[allow(dead_code)]
    pub(super) fn current_kind(&self) -> Kind {
        self.frames.last().expect("frame").kind
    }

    // ---- errors

    pub(super) fn error(&mut self, span: Range<usize>, message: impl Into<String>, incomplete: bool) {
        self.errors.push(ParseError { span, message: message.into(), incomplete });
    }

    /// Error at the next token, or at the end of the statement when there is none.
    pub(super) fn error_here(&mut self, message: impl Into<String>) {
        let incomplete = self.at_end();
        let span = if incomplete && self.pos > 0 { self.prev_span() } else { self.next_span() };
        self.error(span, message, incomplete);
    }

    fn errors_for_unterminated(&mut self) {
        // A string, quoted identifier, comment or dollar quote that runs to the end of the input is an
        // error in every dialect. `incomplete`: it is what the user sees while still typing the closing quote.
        let mut errs = Vec::new();
        for t in &self.raw {
            if !t.terminated {
                let what = match t.kind {
                    TokenKind::String => "string",
                    TokenKind::QuotedIdent => "quoted identifier",
                    // SQLite's tokenizer runs an unterminated comment to the end of the input without complaint.
                    TokenKind::BlockComment if self.fam == Family::Sqlite => continue,
                    TokenKind::BlockComment => "comment",
                    TokenKind::DollarString => "dollar-quoted string",
                    _ => continue,
                };
                errs.push(ParseError {
                    span: t.start..t.end,
                    message: format!("Unterminated {what}"),
                    incomplete: true,
                });
            }
        }
        self.errors.extend(errs);
    }

    // ---- recovery helpers

    /// Consume every remaining token of the statement into an `Unknown` node. A `)` that closes nothing is
    /// reported: parentheses are balanced in every dialect's lexical structure.
    pub(super) fn unknown_rest(&mut self) {
        if self.pos >= self.end {
            return;
        }
        self.start(Kind::Unknown);
        let mut depth = 0i32;
        let mut opens: Vec<Range<usize>> = Vec::new();
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => {
                        depth += 1;
                        opens.push(t.start..t.end);
                    }
                    ")" => {
                        if depth == 0 {
                            self.error(t.start..t.end, "Unmatched ')'", false);
                        } else {
                            depth -= 1;
                            opens.pop();
                        }
                    }
                    _ => {}
                }
            }
            self.bump();
        }
        for o in opens {
            self.error(o, "Unclosed '('", true);
        }
        self.finish();
    }

    /// Consume a balanced group starting at the next token, which must be `(`; stops after its `)`. Everything
    /// goes into the current node. Returns false (after consuming to the end) when the group is not closed.
    pub(super) fn skip_group(&mut self) -> bool {
        let mut depth = 0i32;
        let mut opens: Vec<Range<usize>> = Vec::new();
        while let Some(t) = self.bump() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => {
                        depth += 1;
                        opens.push(t.start..t.end);
                    }
                    ")" => {
                        depth -= 1;
                        opens.pop();
                        if depth <= 0 {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
        }
        // The statement ended inside the group: its parenthesis is never closed.
        for o in opens {
            self.error(o, "Unclosed '('", true);
        }
        false
    }

    /// Consume tokens up to (not including) the `)` that closes the current group, or the end of the
    /// statement. Nested groups are skipped whole. Used when something unexpected sits before a `)`.
    pub(super) fn skip_to_close(&mut self) {
        let mut inner = 0;
        while let Some(t) = self.peek() {
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => inner += 1,
                    ")" if inner == 0 => return,
                    ")" => inner -= 1,
                    _ => {}
                }
            }
            self.bump();
        }
    }

    /// Expect the `)` matching `open` (a span). Anything before it is skipped into an `Unknown` node. If the
    /// statement ends first, that is an error: the opening parenthesis is never closed.
    pub(super) fn expect_close(&mut self, open: Range<usize>) {
        if !self.punct(0, ")") && !self.at_end() {
            self.start(Kind::Unknown);
            self.skip_to_close();
            self.finish();
        }
        if self.punct(0, ")") {
            self.bump();
        } else {
            self.error(open, "Unclosed '('", true);
        }
    }

    // ---- statements

    fn statement(&mut self) {
        let Some(first) = self.peek() else { return };
        if first.kind == TokenKind::Punct && self.text(&first) == "(" {
            self.query();
            return;
        }
        if first.kind != TokenKind::Word {
            self.other();
            return;
        }
        let w = self.text(&first).to_ascii_uppercase();
        if self.end == self.pos + 1
            && matches!(
                w.as_str(),
                "CREATE"
                    | "ALTER"
                    | "DROP"
                    | "DELETE"
                    | "MERGE"
                    | "INSERT"
                    | "UPDATE"
                    | "GRANT"
                    | "REVOKE"
                    | "TRUNCATE"
                    | "RENAME"
            )
        {
            // `CREATE`, `DROP`, `INSERT`, ... with nothing after them is not a statement in any dialect.
            self.error_here(format!("Incomplete {w} statement"));
        }
        match w.as_str() {
            "SELECT" | "VALUES" | "TABLE" => self.query(),
            "FROM" if self.fam == Family::DuckDb => self.query(),
            "WITH" => self.with_statement(),
            "INSERT" | "REPLACE" | "UPSERT" => self.insert(None),
            "UPDATE" if !self.kw(1, "STATISTICS") => self.update(None),
            "DELETE" => self.delete(None),
            "MERGE" => self.merge(None),
            "EXPLAIN" => self.explain(),
            "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" => self.ddl(),
            _ => self.other(),
        }
    }

    /// `WITH ...` followed by a query or by a data-modifying statement that owns the clause.
    fn with_statement(&mut self) {
        self.with_clause();
        let adopt = self.pop_child();
        if self.kw_any(0, &["INSERT", "REPLACE"]) {
            self.insert(adopt);
        } else if self.kw(0, "UPDATE") {
            self.update(adopt);
        } else if self.kw(0, "DELETE") {
            self.delete(adopt);
        } else if self.kw(0, "MERGE") {
            self.merge(adopt);
        } else {
            self.query_after_with(adopt);
        }
    }

    fn explain(&mut self) {
        self.start(Kind::Explain);
        self.bump();
        // Options: `ANALYZE`, `VERBOSE`, `(FORMAT JSON)`, `QUERY PLAN`, `PLAN`, `EXTENDED`. Skip words until a
        // statement keyword appears.
        let mut guard = 0;
        // Did anything beyond the known option words come before the explained statement? (`EXPLAIN tbl`
        // is MySQL's DESCRIBE.)
        let mut foreign_word = false;
        while let Some(t) = self.peek() {
            guard += 1;
            if guard > 40 {
                break;
            }
            if t.kind == TokenKind::Punct && self.text(&t) == "(" {
                if self.paren_starts_query(0) {
                    break;
                }
                self.skip_group();
                continue;
            }
            if t.kind == TokenKind::Word {
                let w = self.text(&t).to_ascii_uppercase();
                if matches!(
                    w.as_str(),
                    "SELECT"
                        | "WITH"
                        | "INSERT"
                        | "UPDATE"
                        | "DELETE"
                        | "MERGE"
                        | "VALUES"
                        | "TABLE"
                        | "CREATE"
                        | "EXECUTE"
                        | "DECLARE"
                        | "REFRESH"
                ) {
                    break;
                }
                if !matches!(
                    w.as_str(),
                    "ANALYZE"
                        | "ANALYSE"
                        | "VERBOSE"
                        | "EXTENDED"
                        | "PARTITIONS"
                        | "FORMAT"
                        | "QUERY"
                        | "PLAN"
                        | "SYNTAX"
                        | "PIPELINE"
                        | "ESTIMATE"
                        | "AST"
                ) {
                    foreign_word = true;
                }
                self.bump();
                continue;
            }
            break;
        }
        if !self.at_end() {
            self.statement();
        } else if !foreign_word && (self.fam != Family::Sqlite || guard == 0) {
            // `EXPLAIN` (with or without options) must explain something. (SQLite explains `ANALYZE` itself.)
            self.error_here("Expected a statement after EXPLAIN");
        }
        self.unknown_rest();
        self.finish();
    }

    /// Any statement known only by its first word. Query-shaped regions after `AS`, `FOR` and `(` are parsed so
    /// that views, `PREPARE`, cursors and `COPY (query)` still resolve names; routine bodies are left alone.
    pub(super) fn other(&mut self) {
        self.start(Kind::Other);
        self.loose_region(true);
        self.finish();
    }

    /// Walk the rest of the statement as opaque tokens, parsing embedded queries when `queries` is set.
    pub(super) fn loose_region(&mut self, queries: bool) {
        let mut depth = 0i32;
        let mut opens: Vec<Range<usize>> = Vec::new();
        let mut prev_intro = false;
        while let Some(t) = self.peek() {
            if queries && self.depth < MAX_DEPTH {
                let intro = prev_intro;
                if intro && self.is_query_start(0) {
                    self.depth += 1;
                    self.query();
                    self.depth -= 1;
                    prev_intro = false;
                    continue;
                }
                if t.kind == TokenKind::Punct && self.text(&t) == "(" && self.paren_starts_query(0) {
                    self.depth += 1;
                    self.subquery();
                    self.depth -= 1;
                    prev_intro = false;
                    continue;
                }
            }
            prev_intro = t.kind == TokenKind::Word
                && ["AS", "FOR", "IS"].iter().any(|k| self.text(&t).eq_ignore_ascii_case(k));
            if t.kind == TokenKind::Punct {
                match self.text(&t) {
                    "(" => {
                        depth += 1;
                        opens.push(t.start..t.end);
                    }
                    ")" => {
                        if depth == 0 {
                            self.error(t.start..t.end, "Unmatched ')'", false);
                        } else {
                            depth -= 1;
                            opens.pop();
                        }
                    }
                    _ => {}
                }
            }
            self.bump();
        }
        for o in opens {
            self.error(o, "Unclosed '('", true);
        }
    }

    // ---- lookahead shared by the grammar modules

    /// Is the token at `n` the start of a query (`SELECT`, `WITH`, `VALUES`, `TABLE`, DuckDB's `FROM`)?
    pub(super) fn is_query_start(&self, n: usize) -> bool {
        if self.kw_any(n, &["SELECT", "WITH", "VALUES"]) {
            return true;
        }
        if self.kw(n, "TABLE") {
            // `TABLE name` as a query body (PostgreSQL); not `TABLE (` and not the `TABLE` of DDL.
            return self.is_ident(n + 1) && self.fam.pg_like();
        }
        self.kw(n, "FROM") && self.fam == Family::DuckDb
    }

    /// Does the `(` at `n` open a query, looking through nested `(`: `((select 1) union (select 2))`?
    pub(super) fn paren_starts_query(&self, n: usize) -> bool {
        if !self.punct(n, "(") {
            return false;
        }
        let mut k = n + 1;
        while self.punct(k, "(") {
            k += 1;
        }
        self.is_query_start(k)
    }
}
