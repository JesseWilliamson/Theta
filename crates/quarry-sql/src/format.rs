//! SQL formatter.
//!
//! The formatter is a pure function of the token sequence: it lays the tokens out
//! with a small Wadler-style document model (groups that stay on one line when they
//! fit `max_line_width`, otherwise break) and never changes anything except
//! whitespace and the case of keywords. Because the output depends only on the
//! tokens, formatting is idempotent by construction; the only inputs taken from the
//! original whitespace are whether a comment sat on its own line, and that choice is
//! reproduced by the output.

use crate::keywords::{is_format_keyword, is_function_like};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::script::{Segment, segments};
use quarry_core::DatabaseKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    pub indent_width: usize,
    pub uppercase_keywords: bool,
    pub max_line_width: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions { indent_width: 4, uppercase_keywords: true, max_line_width: 100 }
    }
}

// ---------------------------------------------------------------------------
// document model

#[derive(Debug, Clone)]
enum Doc {
    Text(String),
    /// A space when the enclosing group is flat, a newline otherwise.
    Line,
    /// Nothing when flat, a newline otherwise.
    SoftLine,
    /// Always a newline; forces every enclosing group to break.
    HardLine,
    Nest(usize, Box<Doc>),
    Group(Box<Doc>),
    Concat(Vec<Doc>),
}

fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}

fn nest(n: usize, d: Doc) -> Doc {
    Doc::Nest(n, Box::new(d))
}

fn group(d: Doc) -> Doc {
    Doc::Group(Box::new(d))
}

fn cat(v: Vec<Doc>) -> Doc {
    Doc::Concat(v)
}

fn fits(mut remaining: isize, doc: &Doc, rest: &[(usize, bool, &Doc)]) -> bool {
    // Work stack for the candidate (flat) followed by what comes after it on the line.
    let mut stack: Vec<(bool, &Doc)> = vec![(true, doc)];
    let mut rest_iter = rest.iter().rev();
    loop {
        let (flat, d) = match stack.pop() {
            Some(x) => x,
            None => match rest_iter.next() {
                Some((_, f, d)) => (*f, *d),
                None => return true,
            },
        };
        match d {
            Doc::Text(s) => {
                // A multi-line token (block comment, long string) ends the current line.
                if let Some((first, _)) = s.split_once('\n') {
                    return remaining - first.chars().count() as isize >= 0;
                }
                remaining -= s.chars().count() as isize;
                if remaining < 0 {
                    return false;
                }
            }
            Doc::Line => {
                if flat {
                    remaining -= 1;
                    if remaining < 0 {
                        return false;
                    }
                } else {
                    return true;
                }
            }
            Doc::SoftLine => {
                if !flat {
                    return true;
                }
            }
            Doc::HardLine => return !flat,
            Doc::Nest(_, x) | Doc::Group(x) => stack.push((flat, x)),
            Doc::Concat(v) => {
                for x in v.iter().rev() {
                    stack.push((flat, x));
                }
            }
        }
    }
}

fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    let mut col: usize = 0;
    // `Some(indent)` after a newline until the next text is written, so that blank
    // runs collapse and no line ever carries trailing spaces.
    let mut pending: Option<usize> = None;
    // Everything up to here came from a token and must be kept byte for byte, even if
    // it ends in whitespace (an unterminated string or comment can).
    let mut protected = 0usize;
    let mut stack: Vec<(usize, bool, &Doc)> = vec![(0, false, doc)];
    while let Some((ind, flat, d)) = stack.pop() {
        match d {
            Doc::Text(s) => {
                if s.is_empty() {
                    continue;
                }
                if let Some(i) = pending.take() {
                    out.push_str(&" ".repeat(i));
                    col = i;
                }
                out.push_str(s);
                protected = out.len();
                col = match s.rfind('\n') {
                    Some(p) => s[p + 1..].chars().count(),
                    None => col + s.chars().count(),
                };
            }
            Doc::Concat(v) => {
                for x in v.iter().rev() {
                    stack.push((ind, flat, x));
                }
            }
            Doc::Nest(n, x) => stack.push((ind + n, flat, x)),
            Doc::Group(x) => {
                let f = flat || fits(width as isize - col as isize, x, &stack);
                stack.push((ind, f, x));
            }
            Doc::Line | Doc::SoftLine | Doc::HardLine => {
                let breaks = matches!(d, Doc::HardLine) || !flat;
                if !breaks {
                    if matches!(d, Doc::Line) {
                        if let Some(i) = pending.take() {
                            out.push_str(&" ".repeat(i));
                        }
                        out.push(' ');
                        col += 1;
                    }
                    continue;
                }
                match pending {
                    Some(_) => pending = Some(ind),
                    None => {
                        while out.len() > protected && out.ends_with(' ') {
                            out.pop();
                        }
                        out.push('\n');
                        pending = Some(ind);
                    }
                }
                col = ind;
            }
        }
    }
    while out.len() > protected && (out.ends_with(' ') || out.ends_with('\n')) {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// tokens and tree

#[derive(Debug, Clone)]
struct Ft {
    kind: TokenKind,
    /// Text to print (keywords already case-adjusted).
    out: String,
    /// Upper-cased text for words, exact text for punctuation/operators.
    up: String,
    /// A newline separated this token from the previous one in the source.
    nl_before: bool,
    /// Used as a keyword (as opposed to an identifier or function name): decides
    /// whether `(` directly after it is a call.
    kw: bool,
    /// An identifier naming a table whose `(` opens a column list, not a call.
    space_paren: bool,
}

#[derive(Debug, Clone)]
enum Node {
    Tok(usize),
    Paren { close: Option<usize>, inner: Vec<Node> },
}

fn build_nodes(toks: &[Ft], pos: &mut usize, stop_at_close: bool) -> Vec<Node> {
    let mut out = Vec::new();
    while *pos < toks.len() {
        let t = &toks[*pos];
        if t.kind == TokenKind::Punct && t.up == "(" {
            *pos += 1;
            let inner = build_nodes(toks, pos, true);
            let close = if *pos < toks.len() && toks[*pos].kind == TokenKind::Punct && toks[*pos].up == ")" {
                let c = *pos;
                *pos += 1;
                Some(c)
            } else {
                None
            };
            out.push(Node::Paren { close, inner });
        } else if t.kind == TokenKind::Punct && t.up == ")" && stop_at_close {
            return out;
        } else {
            out.push(Node::Tok(*pos));
            *pos += 1;
        }
    }
    out
}

struct F<'a> {
    toks: &'a [Ft],
    opts: &'a FormatOptions,
}

#[derive(Clone, Copy)]
struct Edge<'a> {
    kind: TokenKind,
    up: &'a str,
    kw: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CK {
    With,
    Select,
    From,
    Where,
    Having,
    Window,
    GroupBy,
    OrderBy,
    Limit,
    SetOp,
    Insert,
    Values,
    Update,
    Delete,
    Set,
    Returning,
    OnConflict,
    OnDuplicate,
    Join,
    Locking,
}

struct Clause {
    kind: CK,
    /// Node index range of the keyword(s).
    head: std::ops::Range<usize>,
    body: std::ops::Range<usize>,
    /// Comments that sat on their own line right before the keyword.
    leading: std::ops::Range<usize>,
}

const JOIN_MODS: &[&str] = &["NATURAL", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "OUTER", "STRAIGHT_JOIN"];
const STATEMENT_WORDS: &[&str] = &["SELECT", "WITH", "VALUES", "TABLE", "INSERT", "UPDATE", "DELETE"];

impl<'a> F<'a> {
    fn indent(&self) -> usize {
        self.opts.indent_width
    }

    fn tok(&self, n: &Node) -> Option<&Ft> {
        match n {
            Node::Tok(i) => Some(&self.toks[*i]),
            _ => None,
        }
    }

    fn up(&self, nodes: &[Node], i: usize) -> &str {
        match nodes.get(i) {
            Some(Node::Tok(t)) if self.toks[*t].kind == TokenKind::Word => &self.toks[*t].up,
            _ => "",
        }
    }

    fn is_punct(&self, n: &Node, p: &str) -> bool {
        self.tok(n).is_some_and(|t| t.kind == TokenKind::Punct && t.up == p)
    }

    fn is_comment(&self, n: &Node) -> bool {
        self.tok(n).is_some_and(|t| matches!(t.kind, TokenKind::LineComment | TokenKind::BlockComment))
    }

    fn is_line_comment(&self, n: &Node) -> bool {
        self.tok(n).is_some_and(|t| t.kind == TokenKind::LineComment)
    }

    fn edge_left(&self, n: &Node) -> Edge<'_> {
        match n {
            Node::Tok(i) => self.edge_of(*i),
            Node::Paren { .. } => Edge { kind: TokenKind::Punct, up: "(", kw: false },
        }
    }

    fn edge_right(&self, n: &Node) -> Edge<'_> {
        match n {
            Node::Tok(i) => self.edge_of(*i),
            Node::Paren { .. } => Edge { kind: TokenKind::Punct, up: ")", kw: false },
        }
    }

    fn edge_of(&self, i: usize) -> Edge<'_> {
        let t = &self.toks[i];
        // A table name followed by `(` (`INSERT INTO t (a)`) behaves like a keyword for spacing.
        Edge { kind: t.kind, up: &t.up, kw: t.kw || t.space_paren }
    }

    // ----- spacing -----------------------------------------------------

    fn space_between(&self, p: Edge, n: Edge, p_unary: bool) -> bool {
        use TokenKind::*;
        let punct = |e: Edge, s: &str| e.kind == Punct && e.up == s;
        if punct(p, "(") || punct(p, "[") {
            return false;
        }
        if punct(n, ")") || punct(n, "]") || punct(n, ",") || punct(n, ";") {
            return false;
        }
        // `a.b` and `a.*` hug the dot. A dot next to a number or keyword keeps its spaces:
        // `limit .10` would re-lex as one number token.
        if punct(n, ".") {
            return !((p.kind == Word && !p.kw) || p.kind == QuotedIdent || punct(p, ")") || punct(p, "]"));
        }
        if punct(p, ".") {
            return !(matches!(n.kind, Word | QuotedIdent) || (n.kind == Operator && n.up == "*"));
        }
        if (p.kind == Operator && p.up == "::") || (n.kind == Operator && n.up == "::") {
            return false;
        }
        if punct(n, "(") {
            // `count(` / `varchar(` call or type; keywords (`IN (`, `AND (`) keep their space.
            return !((p.kind == Word && !p.kw) || p.kind == QuotedIdent);
        }
        if punct(n, "[") {
            return !((p.kind == Word && (!p.kw || p.up == "ARRAY"))
                || p.kind == QuotedIdent
                || punct(p, ")")
                || punct(p, "]"));
        }
        if p.kind == Operator && n.kind == Operator {
            return true;
        }
        if p_unary {
            return false;
        }
        true
    }

    fn is_unary(&self, op: Edge, prev: Option<Edge>) -> bool {
        if op.kind != TokenKind::Operator || !matches!(op.up, "-" | "+" | "~") {
            return false;
        }
        match prev {
            None => true,
            Some(p) => match p.kind {
                TokenKind::Punct => matches!(p.up, "(" | "[" | "," | ";"),
                TokenKind::Operator => true,
                TokenKind::Word => p.kw,
                _ => false,
            },
        }
    }

    // ----- expressions ---------------------------------------------------

    /// Matching `END` for the CASE at `nodes[i]`.
    fn case_end(&self, nodes: &[Node], i: usize) -> Option<usize> {
        let mut depth = 0;
        for (j, n) in nodes.iter().enumerate().skip(i) {
            match self.up(nodes, j) {
                "CASE" => depth += 1,
                "END" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
            let _ = n;
        }
        None
    }

    /// Inline layout of a node run: spacing rules, parentheses, CASE blocks, comments.
    fn expr(&self, nodes: &[Node]) -> Doc {
        let mut parts: Vec<Doc> = Vec::new();
        let mut prev: Option<Edge> = None;
        let mut prev_unary = false;
        let mut prev_line_comment = false;
        let mut i = 0;
        while i < nodes.len() {
            let (doc, left, right, next_i) = match &nodes[i] {
                Node::Tok(t) if self.toks[*t].up == "CASE" && self.toks[*t].kind == TokenKind::Word => {
                    let end = self.case_end(nodes, i).unwrap_or(nodes.len() - 1);
                    let d = self.case_doc(&nodes[i..=end]);
                    let l = self.edge_of(*t);
                    let r = match &nodes[end] {
                        Node::Tok(e) => self.edge_of(*e),
                        _ => l,
                    };
                    (d, l, r, end + 1)
                }
                Node::Tok(t) => {
                    let ft = &self.toks[*t];
                    let d = if ft.kind == TokenKind::Punct && ft.up == ";" {
                        cat(vec![text(";"), Doc::HardLine])
                    } else {
                        text(ft.out.clone())
                    };
                    (d, self.edge_of(*t), self.edge_of(*t), i + 1)
                }
                Node::Paren { .. } => {
                    (self.paren_doc(&nodes[i]), self.edge_left(&nodes[i]), self.edge_right(&nodes[i]), i + 1)
                }
            };
            if let Some(p) = prev
                && !prev_line_comment
                && self.space_between(p, left, prev_unary)
            {
                parts.push(text(" "));
            }
            let is_lc = self.is_line_comment(&nodes[i]);
            parts.push(doc);
            if is_lc {
                parts.push(Doc::HardLine);
            }
            prev_unary = self.is_unary(left, prev);
            prev = Some(right);
            prev_line_comment = is_lc
                || (matches!(&nodes[i], Node::Tok(t) if self.toks[*t].up == ";" && self.toks[*t].kind == TokenKind::Punct));
            i = next_i;
        }
        cat(parts)
    }

    fn case_doc(&self, nodes: &[Node]) -> Doc {
        // nodes[0] is CASE; nodes.last() is END when the block is closed.
        let closed = self.up(nodes, nodes.len() - 1) == "END" && nodes.len() > 1;
        let body_end = if closed { nodes.len() - 1 } else { nodes.len() };
        let body = &nodes[1..body_end];
        // Split at depth-0 WHEN / ELSE, skipping nested CASE blocks.
        let mut starts: Vec<usize> = Vec::new();
        let mut depth = 0;
        for (i, _) in body.iter().enumerate() {
            match self.up(body, i) {
                "CASE" => depth += 1,
                "END" => depth -= 1,
                "WHEN" | "ELSE" if depth == 0 => starts.push(i),
                _ => {}
            }
        }
        let operand_end = starts.first().copied().unwrap_or(body.len());
        let mut head = vec![text(self.toks_out_of(&nodes[0]))];
        if operand_end > 0 {
            head.push(text(" "));
            head.push(self.expr(&body[..operand_end]));
        }
        let mut arms: Vec<Doc> = Vec::new();
        for (k, &s) in starts.iter().enumerate() {
            let e = starts.get(k + 1).copied().unwrap_or(body.len());
            let arm = &body[s..e];
            if self.up(arm, 0) == "WHEN" {
                let then =
                    (1..arm.len()).find(|&j| self.up(arm, j) == "THEN" && !self.inside_nested_case(arm, j));
                match then {
                    Some(t) => arms.push(group(cat(vec![
                        text(self.toks_out_of(&arm[0])),
                        text(" "),
                        self.cond(&arm[1..t]),
                        nest(
                            self.indent(),
                            cat(vec![
                                Doc::Line,
                                text(self.toks_out_of(&arm[t])),
                                text(" "),
                                self.expr(&arm[t + 1..]),
                            ]),
                        ),
                    ]))),
                    None => arms.push(self.expr(arm)),
                }
            } else {
                arms.push(self.expr(arm));
            }
        }
        // Several arms read better one per line whatever the width.
        let when_count = starts.iter().filter(|&&s| self.up(body, s) == "WHEN").count();
        let sep = if when_count >= 2 { Doc::HardLine } else { Doc::Line };
        let mut inner: Vec<Doc> = Vec::new();
        for a in arms {
            inner.push(sep.clone());
            inner.push(a);
        }
        let mut v = vec![cat(head), nest(self.indent(), cat(inner))];
        if closed {
            v.push(sep);
            v.push(text(self.toks_out_of(&nodes[nodes.len() - 1])));
        }
        group(cat(v))
    }

    fn inside_nested_case(&self, nodes: &[Node], j: usize) -> bool {
        let mut depth = 0;
        for i in 0..j {
            match self.up(nodes, i) {
                "CASE" => depth += 1,
                "END" => depth -= 1,
                _ => {}
            }
        }
        depth > 0
    }

    fn toks_out_of(&self, n: &Node) -> String {
        match n {
            Node::Tok(t) => self.toks[*t].out.clone(),
            Node::Paren { .. } => "(".into(),
        }
    }

    fn split_commas<'n>(&self, nodes: &'n [Node]) -> Vec<&'n [Node]> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, n) in nodes.iter().enumerate() {
            if self.is_punct(n, ",") {
                out.push(&nodes[start..i]);
                start = i + 1;
            }
        }
        out.push(&nodes[start..]);
        out
    }

    /// `a AND b OR c`: one conjunct per line when the enclosing group breaks.
    /// `BETWEEN x AND y` keeps its AND.
    fn cond(&self, nodes: &[Node]) -> Doc {
        let mut splits: Vec<usize> = Vec::new();
        let mut between = false;
        let mut case_depth = 0;
        for i in 0..nodes.len() {
            match self.up(nodes, i) {
                "CASE" => case_depth += 1,
                "END" => case_depth -= 1,
                "BETWEEN" if case_depth == 0 => between = true,
                "AND" if case_depth == 0 => {
                    if between {
                        between = false;
                    } else {
                        splits.push(i);
                    }
                }
                "OR" if case_depth == 0 => splits.push(i),
                _ => {}
            }
        }
        if splits.is_empty() {
            return self.expr(nodes);
        }
        let mut parts = Vec::new();
        let mut start = 0;
        for (k, &s) in splits.iter().enumerate() {
            if s > start || k > 0 {
                if k > 0 {
                    parts.push(Doc::Line);
                }
                parts.push(self.expr(&nodes[start..s]));
            } else if s == 0 {
                // leading operator (half-typed): keep it
            }
            start = s;
        }
        parts.push(Doc::Line);
        parts.push(self.expr(&nodes[start..]));
        cat(parts)
    }

    fn is_subquery(&self, inner: &[Node]) -> bool {
        let mut i = 0;
        while i < inner.len() && (self.is_comment(&inner[i]) || matches!(&inner[i], Node::Paren { .. })) {
            if let Node::Paren { inner: sub, .. } = &inner[i] {
                return self.is_subquery(sub);
            }
            i += 1;
        }
        matches!(self.up(inner, i), "SELECT" | "WITH" | "VALUES" | "INSERT" | "UPDATE" | "DELETE")
    }

    fn paren_doc(&self, n: &Node) -> Doc {
        let Node::Paren { inner, close, .. } = n else { return text("") };
        let close_doc = if close.is_some() { text(")") } else { text("") };
        if inner.is_empty() {
            return cat(vec![text("("), close_doc]);
        }
        let body = if self.is_subquery(inner) {
            self.query(inner, Doc::Line, false)
        } else {
            let items = self.split_commas(inner);
            let mut parts = Vec::new();
            for (k, it) in items.iter().enumerate() {
                if k > 0 {
                    parts.push(text(","));
                    parts.push(Doc::Line);
                }
                parts.push(group(self.cond(it)));
            }
            cat(parts)
        };
        group(cat(vec![
            text("("),
            nest(self.indent(), cat(vec![Doc::SoftLine, body])),
            Doc::SoftLine,
            close_doc,
        ]))
    }

    // ----- clauses ---------------------------------------------------------

    fn clause_kind_at(&self, nodes: &[Node], i: usize, stmt: &str) -> Option<(CK, usize)> {
        let w = self.up(nodes, i);
        if w.is_empty() {
            return None;
        }
        let next = self.up(nodes, i + 1);
        let prev = if i > 0 { self.up(nodes, i - 1) } else { "" };
        // The statement proper begins here: first token (after comments) or right after a WITH list.
        let at_start = nodes[..i].iter().all(|n| self.is_comment(n))
            || (matches!(nodes.get(i - 1), Some(Node::Paren { .. }))
                && nodes
                    .iter()
                    .take(i)
                    .find(|n| !self.is_comment(n))
                    .is_some_and(|n| self.tok(n).is_some_and(|t| t.up == "WITH")));
        match w {
            "WITH" if i == 0 || nodes[..i].iter().all(|n| self.is_comment(n)) => {
                Some((CK::With, if next == "RECURSIVE" { 2 } else { 1 }))
            }
            "SELECT" => {
                let mut len = 1;
                if next == "DISTINCT" {
                    len = 2;
                    if self.up(nodes, i + 2) == "ON" && matches!(nodes.get(i + 3), Some(Node::Paren { .. })) {
                        len = 4;
                    }
                } else if next == "ALL" {
                    len = 2;
                }
                Some((CK::Select, len))
            }
            "FROM" if prev != "DISTINCT" => Some((CK::From, 1)),
            "WHERE" => Some((CK::Where, 1)),
            "HAVING" => Some((CK::Having, 1)),
            "WINDOW" => Some((CK::Window, 1)),
            "GROUP" if next == "BY" => Some((CK::GroupBy, 2)),
            "ORDER" if next == "BY" => Some((CK::OrderBy, 2)),
            "LIMIT" | "OFFSET" | "FETCH" => Some((CK::Limit, 1)),
            "UNION" | "INTERSECT" | "EXCEPT" => {
                Some((CK::SetOp, if matches!(next, "ALL" | "DISTINCT") { 2 } else { 1 }))
            }
            "INSERT" | "REPLACE" if at_start => {
                // INSERT [OR REPLACE | IGNORE] INTO
                let mut len = 1;
                while len < 4 && self.up(nodes, i + len) != "INTO" && i + len < nodes.len() {
                    len += 1;
                }
                if self.up(nodes, i + len) == "INTO" {
                    Some((CK::Insert, len + 1))
                } else {
                    Some((CK::Insert, 1))
                }
            }
            "VALUES" => Some((CK::Values, 1)),
            "UPDATE" if at_start => Some((CK::Update, 1)),
            "DELETE" if at_start => Some((CK::Delete, if next == "FROM" { 2 } else { 1 })),
            "SET" if matches!(stmt, "UPDATE" | "INSERT" | "REPLACE") => Some((CK::Set, 1)),
            "RETURNING" => Some((CK::Returning, 1)),
            "ON" if next == "CONFLICT" => Some((CK::OnConflict, 2)),
            "ON" if next == "DUPLICATE" => Some((
                CK::OnDuplicate,
                if self.up(nodes, i + 2) == "KEY" && self.up(nodes, i + 3) == "UPDATE" { 4 } else { 2 },
            )),
            "USING" if stmt == "DELETE" => Some((CK::From, 1)),
            "FOR" if matches!(next, "UPDATE" | "SHARE" | "NO" | "KEY") => Some((CK::Locking, 1)),
            _ if JOIN_MODS.contains(&w) || w == "JOIN" => {
                let mut j = i;
                while j < nodes.len() && JOIN_MODS.contains(&self.up(nodes, j)) {
                    j += 1;
                }
                if self.up(nodes, j) == "JOIN" {
                    Some((CK::Join, j - i + 1))
                } else if w == "STRAIGHT_JOIN" {
                    Some((CK::Join, 1))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn split_clauses(&self, nodes: &[Node]) -> Vec<Clause> {
        let stmt = {
            let mut i = 0;
            while i < nodes.len() && self.is_comment(&nodes[i]) {
                i += 1;
            }
            self.up(nodes, i).to_string()
        };
        // For `WITH ... <main>` the statement kind is the main statement's.
        let main = if stmt == "WITH" {
            (0..nodes.len())
                .find(|&i| {
                    matches!(self.up(nodes, i), "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "VALUES")
                        && i > 0
                        && matches!(nodes.get(i - 1), Some(Node::Paren { .. }) | None)
                })
                .map(|i| self.up(nodes, i).to_string())
                .unwrap_or(stmt.clone())
        } else {
            stmt.clone()
        };
        let mut starts: Vec<(usize, CK, usize)> = Vec::new();
        let mut i = 0;
        let mut insert_head_done = false;
        while i < nodes.len() {
            // Inside a WITH list the CTE bodies are parens, so depth-0 scanning is safe.
            if let Some((k, len)) = self.clause_kind_at(nodes, i, &main)
                // `values(b)` after ON DUPLICATE KEY UPDATE is a function, not a VALUES list.
                && !(k == CK::Values && !starts.last().is_none_or(|s| matches!(s.1, CK::With | CK::Insert | CK::SetOp)))
                && !(k == CK::Update && !starts.is_empty() && starts.iter().any(|s| s.1 != CK::With))
                && !(k == CK::Insert && insert_head_done)
            {
                if k == CK::Insert {
                    insert_head_done = true;
                }
                starts.push((i, k, len));
                i += len.max(1);
                continue;
            }
            i += 1;
        }
        let mut out = Vec::new();
        for (idx, &(s, k, len)) in starts.iter().enumerate() {
            let end = starts.get(idx + 1).map_or(nodes.len(), |n| n.0);
            let mut body_end = end;
            // Comments on their own line directly before the next keyword lead that clause.
            let mut next_leading = body_end;
            if idx + 1 < starts.len() {
                // (A comment right after a comma would be printed on its own line anyway, so it
                // must lead the next clause on the first pass for the second pass to agree.)
                while next_leading > s + len
                    && self.is_comment(&nodes[next_leading - 1])
                    && (self.tok(&nodes[next_leading - 1]).is_some_and(|t| t.nl_before)
                        || (next_leading >= 2 && self.is_punct(&nodes[next_leading - 2], ",")))
                {
                    next_leading -= 1;
                }
                body_end = next_leading;
            }
            let leading_start = if idx == 0 { 0 } else { out.last().map_or(s, |c: &Clause| c.body.end) };
            let leading = if idx == 0 { 0..s } else { leading_start..s };
            out.push(Clause {
                kind: k,
                head: s..(s + len).min(nodes.len()),
                body: (s + len).min(nodes.len())..body_end.max((s + len).min(nodes.len())),
                leading,
            });
        }
        out
    }

    fn head_doc(&self, nodes: &[Node], r: std::ops::Range<usize>) -> Doc {
        self.expr(&nodes[r])
    }

    fn item_docs(&self, body: &[Node]) -> Vec<Doc> {
        if body.is_empty() {
            return Vec::new();
        }
        self.split_commas(body).iter().map(|it| group(self.cond(it))).collect()
    }

    fn has_comment(&self, body: &[Node]) -> bool {
        body.iter().any(|n| self.is_comment(n))
    }

    /// `KEYWORD item` on one line when a single item fits; otherwise the items on their own lines.
    fn list_clause(&self, head: Doc, items: Vec<Doc>, force_break: bool, has_comment: bool) -> Doc {
        match items.len() {
            0 => head,
            // A comment forces a line break inside the item; keep `SELECT 1 -- note` together.
            1 if has_comment => {
                cat(vec![head, text(" "), nest(self.indent(), items.into_iter().next().unwrap_or(text("")))])
            }
            1 => group(cat(vec![
                head,
                nest(self.indent(), cat(vec![Doc::Line, items.into_iter().next().unwrap_or(text(""))])),
            ])),
            _ => {
                let sep = if force_break { Doc::HardLine } else { Doc::Line };
                let mut inner = Vec::new();
                let n = items.len();
                for (k, it) in items.into_iter().enumerate() {
                    inner.push(sep.clone());
                    inner.push(it);
                    if k + 1 < n {
                        inner.push(text(","));
                    }
                }
                let body = nest(self.indent(), cat(inner));
                if force_break { cat(vec![head, body]) } else { group(cat(vec![head, body])) }
            }
        }
    }

    fn clause_doc(&self, nodes: &[Node], c: &Clause) -> Doc {
        let head = self.head_doc(nodes, c.head.clone());
        let body = &nodes[c.body.clone()];
        match c.kind {
            CK::Select | CK::Returning => {
                self.list_clause(head, self.item_docs(body), true, self.has_comment(body))
            }
            CK::From => {
                let items = self.item_docs(body);
                let force = items.len() > 1;
                // `FROM (` hugs the subquery instead of pushing it to its own line.
                let hug = items.len() == 1
                    && matches!(body.first(), Some(n @ Node::Paren { .. }) if matches!(n, Node::Paren { inner, .. } if self.is_subquery(inner)));
                if hug {
                    cat(vec![head, text(" "), self.expr(body)])
                } else {
                    self.list_clause(head, items, force, self.has_comment(body))
                }
            }
            CK::Where | CK::Having => {
                if body.is_empty() {
                    head
                } else if self.has_comment(body)
                    && !body.iter().any(
                        |n| matches!(n, Node::Tok(t) if matches!(self.toks[*t].up.as_str(), "AND" | "OR")),
                    )
                {
                    cat(vec![head, text(" "), nest(self.indent(), self.cond(body))])
                } else {
                    group(cat(vec![head, nest(self.indent(), cat(vec![Doc::Line, self.cond(body)]))]))
                }
            }
            CK::GroupBy | CK::OrderBy | CK::Window => {
                self.list_clause(head, self.item_docs(body), false, self.has_comment(body))
            }
            CK::Set | CK::OnDuplicate => {
                let items = self.item_docs(body);
                let force = items.len() > 1;
                self.list_clause(head, items, force, self.has_comment(body))
            }
            CK::Values => {
                let items = self.item_docs(body);
                let force = items.len() > 1;
                if items.len() <= 1 {
                    let mut v = vec![head];
                    for it in items {
                        v.push(text(" "));
                        v.push(it);
                    }
                    cat(v)
                } else {
                    self.list_clause(head, items, force, false)
                }
            }
            CK::Limit | CK::Insert | CK::Update | CK::Delete | CK::OnConflict | CK::Locking => {
                if body.is_empty() { head } else { cat(vec![head, text(" "), self.expr(body)]) }
            }
            CK::SetOp => head,
            CK::With => self.with_clause(head, body),
            CK::Join => {
                let on = (0..body.len()).find(|&i| self.up(body, i) == "ON");
                match on {
                    Some(o) => group(cat(vec![
                        head,
                        text(" "),
                        self.expr(&body[..o]),
                        nest(
                            self.indent(),
                            cat(vec![
                                Doc::Line,
                                text(self.toks_out_of(&body[o])),
                                text(" "),
                                self.cond(&body[o + 1..]),
                            ]),
                        ),
                    ])),
                    None if body.is_empty() => head,
                    None => cat(vec![head, text(" "), self.expr(body)]),
                }
            }
        }
    }

    fn with_clause(&self, head: Doc, body: &[Node]) -> Doc {
        let items = self.split_commas(body);
        let mut parts = vec![head];
        for (k, it) in items.iter().enumerate() {
            parts.push(if k == 0 { text(" ") } else { cat(vec![text(","), Doc::HardLine]) });
            // `name [(cols)] AS [MATERIALIZED] ( query )`
            match it.last() {
                Some(Node::Paren { inner, close, .. }) if self.is_subquery(inner) => {
                    let prefix = &it[..it.len() - 1];
                    parts.push(self.expr(prefix));
                    parts.push(text(" ("));
                    parts.push(nest(
                        self.indent(),
                        cat(vec![Doc::HardLine, self.query(inner, Doc::HardLine, false)]),
                    ));
                    parts.push(Doc::HardLine);
                    if close.is_some() {
                        parts.push(text(")"));
                    }
                }
                _ => parts.push(self.expr(it)),
            }
        }
        cat(parts)
    }

    /// A query in clause layout. `sep` separates clauses: a hard line at statement
    /// level, a breakable line inside parentheses so short subqueries stay inline.
    fn query(&self, nodes: &[Node], sep: Doc, top: bool) -> Doc {
        let clauses = self.split_clauses(nodes);
        if clauses.is_empty() {
            return self.generic(nodes, top);
        }
        let mut parts: Vec<Doc> = Vec::new();
        // Anything before the first keyword (leading comments).
        let first = &clauses[0];
        if first.leading.start < first.leading.end {
            parts.push(self.expr(&nodes[first.leading.clone()]));
            if !parts.is_empty() && nodes[first.leading.clone()].iter().any(|n| self.is_comment(n)) {
                parts.push(Doc::HardLine);
            }
        }
        for (k, c) in clauses.iter().enumerate() {
            if k > 0 {
                parts.push(if matches!(c.kind, CK::SetOp) || matches!(clauses[k - 1].kind, CK::SetOp) {
                    Doc::HardLine
                } else {
                    sep.clone()
                });
                if c.leading.start < c.leading.end {
                    parts.push(self.expr(&nodes[c.leading.clone()]));
                    parts.push(Doc::HardLine);
                }
            }
            parts.push(self.clause_doc(nodes, c));
        }
        cat(parts)
    }

    /// Statements without clause structure: DDL, SET, PRAGMA, EXPLAIN wrappers...
    fn generic(&self, nodes: &[Node], _top: bool) -> Doc {
        let mut i = 0;
        while i < nodes.len() && self.is_comment(&nodes[i]) {
            i += 1;
        }
        let first = self.up(nodes, i).to_string();
        // `EXPLAIN [ANALYZE|VERBOSE|(options)] <statement>` and `CREATE ... AS <query>`.
        let split = if first == "EXPLAIN" {
            (i + 1..nodes.len()).find(|&j| STATEMENT_WORDS.contains(&self.up(nodes, j)))
        } else if first == "CREATE" {
            (i + 1..nodes.len().saturating_sub(1))
                .find(|&j| {
                    self.up(nodes, j) == "AS" && matches!(self.up(nodes, j + 1), "SELECT" | "WITH" | "VALUES")
                })
                .map(|j| j + 1)
        } else {
            None
        };
        if let Some(j) = split {
            return cat(vec![
                self.expr(&nodes[..j]),
                Doc::HardLine,
                self.query(&nodes[j..], Doc::HardLine, true),
            ]);
        }
        if first == "CREATE" {
            let table = (i + 1..nodes.len().min(i + 6)).any(|j| self.up(nodes, j) == "TABLE");
            let as_paren = (0..nodes.len()).any(|j| self.up(nodes, j) == "AS");
            if table
                && !as_paren
                && let Some(p) = nodes.iter().position(|n| matches!(n, Node::Paren { .. }))
                && let Node::Paren { inner, close, .. } = &nodes[p]
                && !inner.is_empty()
            {
                let items = self.split_commas(inner);
                let mut body = Vec::new();
                for (k, it) in items.iter().enumerate() {
                    body.push(Doc::HardLine);
                    body.push(self.expr(it));
                    if k + 1 < items.len() {
                        body.push(text(","));
                    }
                }
                let mut v =
                    vec![self.expr(&nodes[..p]), text(" ("), nest(self.indent(), cat(body)), Doc::HardLine];
                if close.is_some() {
                    v.push(text(")"));
                }
                if p + 1 < nodes.len() {
                    v.push(text(" "));
                    v.push(self.expr(&nodes[p + 1..]));
                }
                return cat(v);
            }
        }
        self.expr(nodes)
    }

    fn statement(&self, nodes: &[Node]) -> Doc {
        let mut i = 0;
        while i < nodes.len() && self.is_comment(&nodes[i]) {
            i += 1;
        }
        let clause_like = matches!(
            self.up(nodes, i),
            "SELECT" | "WITH" | "INSERT" | "REPLACE" | "UPDATE" | "DELETE" | "VALUES"
        ) || matches!(nodes.get(i), Some(Node::Paren { inner, .. }) if self.is_subquery(inner));
        if clause_like { self.query(nodes, Doc::HardLine, true) } else { self.generic(nodes, true) }
    }
}

// ---------------------------------------------------------------------------
// driver

fn adjust_case(toks: &mut [Ft], upper: bool) {
    for i in 0..toks.len() {
        if toks[i].kind != TokenKind::Word {
            continue;
        }
        let up = toks[i].up.clone();
        let prev = if i > 0 { toks[i - 1].up.as_str() } else { "" };
        let prev_is_word = i > 0 && toks[i - 1].kind == TokenKind::Word;
        let prev_dot = i > 0 && toks[i - 1].kind == TokenKind::Punct && toks[i - 1].up == ".";
        let next = toks.get(i + 1);
        let next_up = next.map_or("", |t| t.up.as_str());
        let next_is_word = next.is_some_and(|t| t.kind == TokenKind::Word);
        let next_dot = next.is_some_and(|t| t.kind == TokenKind::Punct && t.up == ".");
        let next_paren = next.is_some_and(|t| t.kind == TokenKind::Punct && t.up == "(");
        let after_as = prev_is_word && prev == "AS";
        let stmt_word =
            matches!(up.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE" | "INSERT" | "UPDATE" | "DELETE");
        let mut kw = is_format_keyword(&up)
            && !prev_dot
            && !next_dot
            && !(is_function_like(&up) && next_paren)
            && !(after_as && !stmt_word);
        // Words that are keywords only in context.
        if !kw && !prev_dot && !next_dot {
            kw = match up.as_str() {
                "KEY" => {
                    prev_is_word && matches!(prev, "PRIMARY" | "FOREIGN" | "UNIQUE")
                        || (next_is_word && next_up == "UPDATE" && prev == "DUPLICATE")
                }
                "ROWS" | "RANGE" | "GROUPS" => {
                    next_is_word && matches!(next_up, "BETWEEN" | "UNBOUNDED" | "CURRENT")
                }
                "CURRENT" => next_is_word && next_up == "ROW",
                "ROW" => prev_is_word && prev == "CURRENT",
                "FIRST" | "LAST" => prev_is_word && prev == "NULLS",
                "DUPLICATE" => prev_is_word && prev == "ON",
                "CONFLICT" => prev_is_word && prev == "ON",
                _ => false,
            };
        }
        toks[i].kw = kw;
        if upper && kw {
            toks[i].out = up;
        }
    }
    // `INSERT INTO t (a)`, `CREATE TABLE t (...)`, `REFERENCES t (id)`, `ON t (col)`: not calls.
    for i in 1..toks.len() {
        let is_paren = toks[i].kind == TokenKind::Punct && toks[i].up == "(";
        if !is_paren || !matches!(toks[i - 1].kind, TokenKind::Word | TokenKind::QuotedIdent) {
            continue;
        }
        let mut s = i - 1;
        while s >= 2
            && toks[s - 1].kind == TokenKind::Punct
            && toks[s - 1].up == "."
            && matches!(toks[s - 2].kind, TokenKind::Word | TokenKind::QuotedIdent)
        {
            s -= 2;
        }
        if s == 0 {
            continue;
        }
        let before = toks[s - 1].up.as_str();
        let create = toks[0].up == "CREATE";
        if toks[s - 1].kind == TokenKind::Word
            && (matches!(before, "INTO" | "TABLE" | "REFERENCES" | "ONLY" | "COPY")
                || (before == "ON" && create)
                || (before == "EXISTS" && create))
        {
            toks[i - 1].space_paren = true;
        }
    }
}

fn make_tokens(sql: &str, raw: &[Token], opts: &FormatOptions) -> Vec<Ft> {
    let mut out: Vec<Ft> = Vec::new();
    let mut nl = false;
    for t in raw {
        match t.kind {
            TokenKind::Whitespace => {
                nl |= t.text(sql).contains('\n');
            }
            _ => {
                let txt = t.text(sql);
                let up = match t.kind {
                    TokenKind::Word => txt.to_ascii_uppercase(),
                    TokenKind::Punct | TokenKind::Operator => txt.to_string(),
                    _ => String::new(),
                };
                let nl_before =
                    nl || (!out.is_empty() && out.last().is_some_and(|p| p.kind == TokenKind::LineComment));
                out.push(Ft {
                    kind: t.kind,
                    out: txt.to_string(),
                    up,
                    nl_before,
                    kw: false,
                    space_paren: false,
                });
                nl = false;
            }
        }
    }
    adjust_case(&mut out, opts.uppercase_keywords);
    out
}

fn format_segment(sql: &str, raw: &[Token], seg: &Segment, opts: &FormatOptions) -> Option<String> {
    let toks = make_tokens(sql, &raw[seg.tokens.clone()], opts);
    if toks.is_empty() {
        return None;
    }
    let mut pos = 0;
    let nodes = build_nodes(&toks, &mut pos, false);
    // A stray `)` stops the first pass; keep consuming so no token is ever dropped.
    let mut nodes = nodes;
    while pos < toks.len() {
        nodes.push(Node::Tok(pos));
        pos += 1;
        nodes.extend(build_nodes(&toks, &mut pos, false));
    }
    let f = F { toks: &toks, opts };
    let only_comments = nodes.iter().all(|n| f.is_comment(n));
    let doc = if only_comments {
        let mut parts = Vec::new();
        for (k, n) in nodes.iter().enumerate() {
            if k > 0 {
                parts.push(Doc::HardLine);
            }
            parts.push(text(f.toks_out_of(n)));
        }
        cat(parts)
    } else {
        f.statement(&nodes)
    };
    let mut s = render(&doc, opts.max_line_width);
    if seg.has_semi {
        // A trailing line comment would swallow the terminator if it shared its line.
        let last_is_line_comment = nodes.last().is_some_and(|n| f.is_line_comment(n));
        if last_is_line_comment {
            s.push('\n');
        }
        s.push(';');
    }
    Some(s)
}

/// Re-indent and re-case a script. Only whitespace and keyword case change.
pub fn format(sql: &str, kind: DatabaseKind, options: &FormatOptions) -> String {
    let raw = tokenize(sql, kind);
    let segs = segments(sql, &raw, kind);
    let mut parts: Vec<String> = Vec::new();
    for seg in &segs {
        if let Some(s) = format_segment(sql, &raw, seg, options) {
            parts.push(s);
        } else if seg.has_semi {
            // A lone `;` (empty statement): keep it so no token disappears.
            parts.push(";".into());
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use DatabaseKind::*;

    fn fmt(sql: &str, kind: DatabaseKind) -> String {
        format(sql, kind, &FormatOptions::default())
    }

    fn pg(sql: &str) -> String {
        fmt(sql, Postgres)
    }

    /// Significant tokens of `sql` as (kind, text).
    fn sig(sql: &str, kind: DatabaseKind) -> Vec<(TokenKind, String)> {
        tokenize(sql, kind)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .map(|t| (t.kind, t.text(sql).to_string()))
            .collect()
    }

    /// Formatting may change whitespace and the case of keywords, nothing else.
    fn assert_same_tokens(input: &str, output: &str, kind: DatabaseKind) {
        let (a, b) = (sig(input, kind), sig(output, kind));
        assert_eq!(a.len(), b.len(), "token count changed\n--- input\n{input}\n--- output\n{output}");
        for ((ka, ta), (kb, tb)) in a.iter().zip(&b) {
            assert_eq!(ka, kb, "kind changed: {ta:?} -> {tb:?}\n{output}");
            if ta != tb {
                let up = ta.to_ascii_uppercase();
                assert!(*ka == TokenKind::Word && up == *tb, "token changed: {ta:?} -> {tb:?}\n{output}");
                let ctx = matches!(
                    up.as_str(),
                    "KEY"
                        | "ROWS"
                        | "RANGE"
                        | "GROUPS"
                        | "CURRENT"
                        | "ROW"
                        | "FIRST"
                        | "LAST"
                        | "DUPLICATE"
                        | "CONFLICT"
                );
                assert!(is_format_keyword(ta) || ctx, "non-keyword changed case: {ta:?} -> {tb:?}");
            }
        }
    }

    const CORPUS: &[(&str, DatabaseKind)] = &[
        ("select 1", Postgres),
        (
            "select c.id, c.name, count(o.id) as n from customers c left join orders o on o.customer_id = c.id where c.tier = 'gold' group by c.id, c.name having count(o.id) > 2 order by n desc limit 10",
            Postgres,
        ),
        (
            "with recent as (select id, total from orders where placed_at > now() - interval '7 days'), big as (select * from recent where total > 100) select r.id from recent r join big b on b.id = r.id",
            Postgres,
        ),
        (
            "with recursive t(n) as (select 1 union all select n + 1 from t where n < 5) select * from t",
            Postgres,
        ),
        (
            "select a, row_number() over (partition by a order by b desc) as rn, sum(x) over w from t window w as (partition by a)",
            Postgres,
        ),
        (
            "select case when a > 1 then 'x' when a > 0 then 'y' else 'z' end as c, case b when 1 then 'one' end from t",
            Postgres,
        ),
        ("select data->>'k', data #>> '{a,b}', x::int, y::numeric(10,2), arr[1], arr[1:2] from t", Postgres),
        ("select -1, - 2, 3 - -4, a - b, a-b, +5, ~6 from t", Postgres),
        (
            "insert into customers (id, email) values (1, 'a'), (2, 'b') on conflict (id) do update set email = excluded.email returning id",
            Postgres,
        ),
        ("insert into t select * from u where x = 1", Postgres),
        ("insert into t (a) values (1) on conflict do nothing", Postgres),
        (
            "update orders o set status = 'paid', total = total * 2 from customers c where o.customer_id = c.id and c.tier = 'x'",
            Postgres,
        ),
        ("delete from orders where id in (select order_id from order_items where quantity = 0)", Postgres),
        ("with d as (delete from t where a = 1 returning *) insert into log select * from d", Postgres),
        ("with u as (update t set a = 1 where b = 2 returning id) select * from u", Postgres),
        (
            "select * from (select a, b from t where c = 1) s where s.a in (1,2,3) and exists (select 1 from u where u.a = s.a)",
            Postgres,
        ),
        (
            "select * from t where a between 1 and 10 and b not like '%x' or c is not null and d is distinct from e",
            Postgres,
        ),
        ("select * from a union all select * from b union select * from c order by 1", Postgres),
        ("select distinct on (a) a, b from t order by a, b desc", Postgres),
        ("select count(*) filter (where x > 1), string_agg(distinct a, ',' order by a) from t", Postgres),
        (
            "create table t (id serial primary key, name text not null default '', c timestamptz default now(), constraint fk foreign key (id) references u(id) on delete cascade)",
            Postgres,
        ),
        ("create index idx on t using gin (a, b) where c > 1", Postgres),
        ("alter table t add column x int default 0, drop column y", Postgres),
        ("create view v as select a, b from t where c > 1", Postgres),
        ("explain (analyze, buffers) select * from t where x = 1", Postgres),
        ("create function f(a int) returns int as $$ begin return a + 1; end; $$ language plpgsql", Postgres),
        ("do $body$ begin perform 1; end $body$", Postgres),
        ("select E'a\\'b', 'it''s', \"Quoted Ident\", $1, $2::text from t", Postgres),
        ("select * from t -- trailing\nwhere /* inline */ a = 1 -- more\n", Postgres),
        ("-- leading comment\nselect a, -- first\n b /* second */ from t", Postgres),
        ("select a from t where b = 1\n-- before order\norder by a", Postgres),
        ("select 1; select 2;;\n\n-- end comment", Postgres),
        ("select `order`.id, ifnull(a, 0) from `order` where name = \"x\" # note\nlimit 5, 10", MySql),
        ("insert into t (a, b) values (1, 2) on duplicate key update b = values(b), a = a + 1", MySql),
        ("update t join u on t.id = u.id set t.a = u.a where u.b = 1", MySql),
        ("select * from t left outer join u on t.a = u.a straight_join v on v.a = u.a", MySql),
        ("create procedure p() begin select 1; update t set a = 1; end", MySql),
        ("select [my col], ?1, :name from t where x glob 'a*'", Sqlite),
        ("insert or replace into t (a) values (1)", Sqlite),
        ("create trigger tr after insert on t begin update u set a = 1; end", Sqlite),
        ("select * from t where a = 'unterminated", Postgres),
        ("select a, from t where", Postgres),
        ("select (1 + 2)) from t", Postgres),
    ];

    #[test]
    fn corpus_is_idempotent_and_preserves_tokens() {
        assert!(CORPUS.len() >= 25);
        for &(sql, kind) in CORPUS {
            for opts in [
                FormatOptions::default(),
                FormatOptions { indent_width: 2, uppercase_keywords: false, max_line_width: 60 },
                FormatOptions { indent_width: 8, uppercase_keywords: true, max_line_width: 200 },
                FormatOptions { indent_width: 4, uppercase_keywords: true, max_line_width: 10 },
            ] {
                let once = format(sql, kind, &opts);
                assert_same_tokens(sql, &once, kind);
                let twice = format(&once, kind, &opts);
                assert_eq!(once, twice, "not idempotent for {sql:?} with {opts:?}");
                assert!(!once.lines().any(|l| l.ends_with(' ')), "trailing whitespace in\n{once}");
            }
        }
    }

    #[test]
    fn basic_select_layout() {
        let out = pg(
            "select c.id, c.name, count(o.id) as n from customers c left join orders o on o.customer_id = c.id and o.status = 'paid' where c.tier in ('gold','silver') group by c.id, c.name having count(o.id) > 2 order by n desc limit 10",
        );
        assert_eq!(
            out,
            "SELECT\n    c.id,\n    c.name,\n    count(o.id) AS n\nFROM customers c\nLEFT JOIN orders o ON o.customer_id = c.id AND o.status = 'paid'\nWHERE c.tier IN ('gold', 'silver')\nGROUP BY c.id, c.name\nHAVING count(o.id) > 2\nORDER BY n DESC\nLIMIT 10"
        );
    }

    #[test]
    fn short_statements_stay_on_one_line() {
        assert_eq!(pg("select 1"), "SELECT 1");
        assert_eq!(pg("select * from t where a=1"), "SELECT *\nFROM t\nWHERE a = 1");
        assert_eq!(pg("delete from t where id=1"), "DELETE FROM t\nWHERE id = 1");
    }

    #[test]
    fn where_breaks_conjuncts_when_too_long() {
        let out = fmt(
            "select * from t where aaaaaaaaaaaaaaaaaaaa = 1 and bbbbbbbbbbbbbbbbbbbbbbbb = 2 and cccccccccccccccccccccccc = 3 and dddddddddddd = 4",
            Postgres,
        );
        assert_eq!(
            out,
            "SELECT *\nFROM t\nWHERE\n    aaaaaaaaaaaaaaaaaaaa = 1\n    AND bbbbbbbbbbbbbbbbbbbbbbbb = 2\n    AND cccccccccccccccccccccccc = 3\n    AND dddddddddddd = 4"
        );
    }

    #[test]
    fn between_and_is_not_split() {
        let out = fmt(
            "select * from t where aaaaaaaaaaaaaaaaaaaaaaaaa between 1 and 100000000 and bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb = 2 and c = 3",
            Postgres,
        );
        assert!(out.contains("BETWEEN 1 AND 100000000\n"), "{out}");
        assert!(out.contains("\n    AND bbbbbbb"));
    }

    #[test]
    fn cte_layout() {
        let out = pg("with a as (select 1 as x), b as (select x from a) select * from b");
        assert_eq!(
            out,
            "WITH a AS (\n    SELECT 1 AS x\n),\nb AS (\n    SELECT x\n    FROM a\n)\nSELECT *\nFROM b"
        );
    }

    #[test]
    fn case_expressions() {
        let out = pg("select case when a > 1 then 'x' when a > 0 then 'y' else 'z' end as c from t");
        assert_eq!(
            out,
            "SELECT\n    CASE\n        WHEN a > 1 THEN 'x'\n        WHEN a > 0 THEN 'y'\n        ELSE 'z'\n    END AS c\nFROM t"
        );
        let out = pg("select case when a then 1 else 0 end from t");
        assert!(out.starts_with("SELECT CASE WHEN a THEN 1 ELSE 0 END\n"), "{out}");
    }

    #[test]
    fn postgres_casts_and_json_operators() {
        let out = pg("select data->>'k', x::int, y::numeric(10,2), (a+b)::text, arr[1] from t");
        assert!(out.contains("data ->> 'k'"), "{out}");
        assert!(out.contains("x::int"));
        assert!(out.contains("y::numeric(10, 2)"));
        assert!(out.contains("(a + b)::text"));
        assert!(out.contains("arr[1]"));
    }

    #[test]
    fn unary_and_binary_minus() {
        assert_eq!(pg("select -1, 3 - -4, a-b, - 2"), "SELECT\n    -1,\n    3 - -4,\n    a - b,\n    -2");
    }

    #[test]
    fn insert_on_conflict() {
        let out = pg(
            "insert into customers (id, email) values (1, 'a'), (2, 'b') on conflict (id) do update set email = excluded.email returning id",
        );
        assert_eq!(
            out,
            "INSERT INTO customers (id, email)\nVALUES\n    (1, 'a'),\n    (2, 'b')\nON CONFLICT (id) DO UPDATE\nSET email = excluded.email\nRETURNING id"
        );
        assert_eq!(pg("insert into t (a) values (1)"), "INSERT INTO t (a)\nVALUES (1)");
    }

    #[test]
    fn update_with_multiple_assignments() {
        let out = pg("update orders set status = 'paid', total = total * 2 where id = 7");
        assert_eq!(out, "UPDATE orders\nSET\n    status = 'paid',\n    total = total * 2\nWHERE id = 7");
    }

    #[test]
    fn data_modifying_cte_main_statement() {
        let out = pg("with d as (delete from t where a = 1 returning *) insert into log select * from d");
        assert_eq!(
            out,
            "WITH d AS (\n    DELETE FROM t\n    WHERE a = 1\n    RETURNING *\n)\nINSERT INTO log\nSELECT *\nFROM d"
        );
    }

    #[test]
    fn subqueries_inline_when_short_and_hug_from() {
        let out = pg("select * from t where id in (select id from u where x = 1)");
        assert_eq!(out, "SELECT *\nFROM t\nWHERE id IN (SELECT id FROM u WHERE x = 1)");
        let out = pg("select * from (select a, b from t) s");
        assert_eq!(out, "SELECT *\nFROM (\n    SELECT\n        a,\n        b\n    FROM t\n) s");
    }

    #[test]
    fn set_operations() {
        assert_eq!(
            pg("select a from t union all select b from u"),
            "SELECT a\nFROM t\nUNION ALL\nSELECT b\nFROM u"
        );
    }

    #[test]
    fn window_functions() {
        let out = pg("select a, row_number() over (partition by a order by b desc) as rn from t");
        assert!(out.contains("row_number() OVER (PARTITION BY a ORDER BY b DESC) AS rn"), "{out}");
        let out =
            pg("select sum(x) over (order by d rows between unbounded preceding and current row) from t");
        assert!(out.contains("ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW"), "{out}");
    }

    #[test]
    fn create_table_one_column_per_line() {
        let out = pg(
            "create table t (id serial primary key, name text not null, constraint fk foreign key (id) references u(id))",
        );
        assert_eq!(
            out,
            "CREATE TABLE t (\n    id serial PRIMARY KEY,\n    name text NOT NULL,\n    CONSTRAINT fk FOREIGN KEY (id) REFERENCES u (id)\n)"
        );
    }

    #[test]
    fn create_view_and_explain_wrap_queries() {
        assert_eq!(pg("create view v as select a from t"), "CREATE VIEW v AS\nSELECT a\nFROM t");
        assert_eq!(pg("explain analyze select * from t"), "EXPLAIN ANALYZE\nSELECT *\nFROM t");
    }

    #[test]
    fn dollar_quoted_bodies_are_untouched() {
        let sql = "create function f() returns int as $$\nbegin\n    select   1;\nend;\n$$ language plpgsql";
        let out = pg(sql);
        assert!(out.contains("$$\nbegin\n    select   1;\nend;\n$$"), "{out}");
        assert!(out.starts_with("CREATE FUNCTION f() RETURNS int AS $$"));
        assert!(out.ends_with("LANGUAGE plpgsql"));
    }

    #[test]
    fn string_literals_and_identifiers_keep_their_case() {
        let out = pg("select 'Select From Where', \"Select\" from \"Table\" where \"Order\" = 'And'");
        assert!(
            out.contains("'Select From Where'")
                && out.contains("\"Select\"")
                && out.contains("\"Table\"")
                && out.contains("\"Order\"")
        );
        assert!(out.contains("'And'"));
    }

    #[test]
    fn identifiers_that_look_like_keywords_are_left_alone() {
        let out = pg("select t.order, t.group, status, name, key, value from t");
        assert!(out.contains("t.order") && out.contains("t.group"), "{out}");
        assert!(
            out.contains("status") && out.contains("name") && out.contains("key") && out.contains("value")
        );
        let out = pg("select x as end, left(a, 2), right(b, 1) from t");
        assert!(out.contains("AS end") && out.contains("left(a, 2)") && out.contains("right(b, 1)"), "{out}");
        let out = pg("select * from a left join b on a.x = b.x");
        assert!(out.contains("LEFT JOIN"));
    }

    #[test]
    fn comments_survive() {
        let out = pg("-- header\nselect a, -- first\n b /* second */ from t -- tail\nwhere x = 1");
        assert!(out.starts_with("-- header\nSELECT"), "{out}");
        for c in ["-- first", "/* second */", "-- tail", "-- header"] {
            assert!(out.contains(c), "{c} missing in\n{out}");
        }
        // A line comment never swallows following code.
        for line in out.lines() {
            if let Some(pos) = line.find("--") {
                assert!(!line[pos..].contains("FROM") && !line[pos..].contains("WHERE"), "{line}");
            }
        }
    }

    #[test]
    fn own_line_comment_before_clause_stays_on_its_own_line() {
        let out = pg("select a from t\n-- keep rows\nwhere a = 1");
        assert_eq!(out, "SELECT a\nFROM t\n-- keep rows\nWHERE a = 1");
    }

    #[test]
    fn trailing_line_comment_before_semicolon() {
        let out = pg("select 1 -- c\n;");
        assert_eq!(out, "SELECT 1 -- c\n;");
        assert_eq!(pg(&out), out);
    }

    #[test]
    fn multiple_statements_are_separated_by_blank_lines() {
        assert_eq!(pg("select 1;select 2;"), "SELECT 1;\n\nSELECT 2;");
        assert_eq!(pg("select 1;\n\n\n-- end"), "SELECT 1;\n\n-- end");
        assert_eq!(pg(""), "");
        assert_eq!(pg("   \n  "), "");
    }

    #[test]
    fn options_are_respected() {
        let lower = format(
            "SELECT a FROM t WHERE b = 1",
            Postgres,
            &FormatOptions { uppercase_keywords: false, ..Default::default() },
        );
        assert_eq!(lower, "SELECT a\nFROM t\nWHERE b = 1", "case is left as written");
        let two =
            format("select a, b from t", Postgres, &FormatOptions { indent_width: 2, ..Default::default() });
        assert_eq!(two, "SELECT\n  a,\n  b\nFROM t");
        let narrow = format(
            "select * from t where a = 1 and b = 2",
            Postgres,
            &FormatOptions { max_line_width: 20, ..Default::default() },
        );
        assert_eq!(narrow, "SELECT *\nFROM t\nWHERE\n    a = 1\n    AND b = 2");
    }

    #[test]
    fn default_options() {
        assert_eq!(
            FormatOptions::default(),
            FormatOptions { indent_width: 4, uppercase_keywords: true, max_line_width: 100 }
        );
    }

    #[test]
    fn mysql_specifics() {
        let out =
            fmt("insert into t (a, b) values (1, 2) on duplicate key update b = values(b), a = a + 1", MySql);
        assert_eq!(
            out,
            "INSERT INTO t (a, b)\nVALUES (1, 2)\nON DUPLICATE KEY UPDATE\n    b = VALUES (b),\n    a = a + 1"
        );
        let out = fmt("select `order`.id from `order` # note\nwhere x = 1", MySql);
        assert!(out.contains("`order`.id") && out.contains("# note"));
    }

    #[test]
    fn sqlite_specifics() {
        let out = fmt("select [my col], ?1 from t where x glob 'a*'", Sqlite);
        assert!(out.contains("[my col]") && out.contains("?1"));
        assert_eq!(
            fmt("insert or replace into t (a) values (1)", Sqlite),
            "INSERT OR REPLACE INTO t (a)\nVALUES (1)"
        );
    }

    #[test]
    fn invalid_sql_is_laid_out_without_losing_tokens() {
        for sql in [
            "select a, from t where",
            "select (1 + 2)) from t",
            "select 'abc",
            "select /* open",
            "update",
            ")",
            "(((",
            "select * from where and or",
        ] {
            let out = pg(sql);
            assert_same_tokens(sql, &out, Postgres);
            assert_eq!(pg(&out), out, "{sql}");
        }
    }

    #[test]
    fn long_in_list_breaks_one_per_line() {
        let items: Vec<String> = (0..40).map(|i| format!("'value_{i}'")).collect();
        let out = pg(&format!("select * from t where x in ({})", items.join(",")));
        assert!(out.lines().count() > 40, "{out}");
        assert!(out.lines().all(|l| l.chars().count() <= 100), "{out}");
    }

    #[test]
    fn long_select_item_function_args_wrap() {
        let out = pg(
            "select coalesce(aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, cccccccccccccccccccccccccccc, dddddddddddddddddddd) as x from t",
        );
        assert!(out.lines().all(|l| l.chars().count() <= 100), "{out}");
        assert!(out.contains("coalesce(\n"));
    }
}
