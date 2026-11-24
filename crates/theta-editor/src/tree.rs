//! A small structural tree of a SQL buffer, for Helix's syntax-node commands
//! (`alt-o` expand, `alt-i` shrink, `alt-p` / `alt-n` siblings, `alt-e` / `alt-b` parent edges).
//!
//! Why not tree-sitter: the editor has no parser dependency (see `syntax.rs`) and half-typed
//! statements are the normal case in a console, so the tree is built by a forgiving scanner that
//! always produces *something*: an unclosed `(` runs to the end of its statement, a stray `)` is an
//! ordinary token. The levels, from the outside in:
//!
//! * the buffer (first token to last token);
//! * statements, split on `;`;
//! * clauses, started by `select`, `from`, `where`, `group`, `order` ... at bracket depth 0;
//! * comma separated items of a clause, or of a bracket group;
//! * calls (`name(...)`), bracket groups, dotted names (`a.b.c`) and single tokens.
//!
//! Nodes whose range equals their parent's are folded away, as expansion would skip them anyway.

use crate::text::Text;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tok {
    Word,
    Quoted,
    Number,
    Open,
    Close,
    Comma,
    Semi,
    Dot,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Token {
    kind: Tok,
    from: usize,
    to: usize,
    /// A keyword that starts a clause (`select`, `from`, `left` ...).
    clause: bool,
    /// A word that continues a clause keyword (`by`, `into`, `all`, `join` ...).
    cont: bool,
}

#[derive(Clone, Debug)]
struct Node {
    from: usize,
    to: usize,
    parent: Option<usize>,
    children: Vec<usize>,
}

pub(crate) struct Tree {
    nodes: Vec<Node>,
    root: usize,
}

/// A range in char offsets, `to` exclusive.
pub(crate) type Span = (usize, usize);

const CLAUSE_WORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "group",
    "having",
    "order",
    "limit",
    "offset",
    "union",
    "intersect",
    "except",
    "with",
    "insert",
    "values",
    "set",
    "update",
    "delete",
    "returning",
    "window",
    "join",
    "on",
    "using",
    "fetch",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
];

/// Words that extend the keyword that opened a clause: `group BY`, `insert INTO`, `union ALL`, `left OUTER JOIN`.
const CONTINUATION_WORDS: &[&str] =
    &["by", "into", "all", "distinct", "outer", "join", "inner", "left", "right", "full", "cross", "natural"];

fn lex(chars: &[char]) -> Vec<Token> {
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(n);
        } else if c == '\'' || c == '"' || c == '`' {
            let start = i;
            i += 1;
            while i < n {
                if chars[i] == c {
                    if chars.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(Token { kind: Tok::Quoted, from: start, to: i, clause: false, cont: false });
        } else if is_word(c) {
            let start = i;
            while i < n && is_word(chars[i]) {
                i += 1;
            }
            let kind = if chars[start].is_ascii_digit() { Tok::Number } else { Tok::Word };
            let word: String = chars[start..i].iter().collect::<String>().to_lowercase();
            let is_word_kind = kind == Tok::Word;
            let clause = is_word_kind && CLAUSE_WORDS.contains(&word.as_str());
            let cont = is_word_kind && CONTINUATION_WORDS.contains(&word.as_str());
            out.push(Token { kind, from: start, to: i, clause, cont });
        } else {
            let kind = match c {
                '(' | '[' | '{' => Tok::Open,
                ')' | ']' | '}' => Tok::Close,
                ',' => Tok::Comma,
                ';' => Tok::Semi,
                '.' => Tok::Dot,
                _ => Tok::Other,
            };
            out.push(Token { kind, from: i, to: i + 1, clause: false, cont: false });
            i += 1;
        }
    }
    out
}

struct Builder<'a> {
    toks: &'a [Token],
    nodes: Vec<Node>,
}

impl Builder<'_> {
    fn node(&mut self, from: usize, to: usize, children: Vec<usize>) -> usize {
        let id = self.nodes.len();
        for &c in &children {
            self.nodes[c].parent = Some(id);
        }
        self.nodes.push(Node { from, to, parent: None, children });
        id
    }

    /// One atom at `*i`: a token, a dotted name, a call or a bracket group. `limit` bounds the scan
    /// (the end of the enclosing statement) for unclosed groups.
    fn atom(&mut self, i: &mut usize, limit: usize) -> usize {
        let t = self.toks[*i];
        match t.kind {
            Tok::Open => self.group(i, limit),
            Tok::Word | Tok::Quoted | Tok::Number => {
                let mut parts = vec![self.node(t.from, t.to, vec![])];
                *i += 1;
                // a.b.c
                while *i + 1 < limit
                    && self.toks[*i].kind == Tok::Dot
                    && self.toks[*i].from == self.toks[*i - 1].to
                    && matches!(self.toks[*i + 1].kind, Tok::Word | Tok::Quoted | Tok::Number)
                    && self.toks[*i + 1].from == self.toks[*i].to
                {
                    let w = self.toks[*i + 1];
                    parts.push(self.node(w.from, w.to, vec![]));
                    *i += 2;
                }
                let mut name = if parts.len() == 1 {
                    parts[0]
                } else {
                    let (from, to) = (self.nodes[parts[0]].from, self.nodes[*parts.last().unwrap()].to);
                    self.node(from, to, parts)
                };
                // name(...)
                if *i < limit && self.toks[*i].kind == Tok::Open && self.toks[*i].from == self.nodes[name].to {
                    let g = self.group(i, limit);
                    let (from, to) = (self.nodes[name].from, self.nodes[g].to);
                    name = self.node(from, to, vec![name, g]);
                }
                name
            }
            _ => {
                *i += 1;
                self.node(t.from, t.to, vec![])
            }
        }
    }

    /// `( items )`; the brackets themselves are not nodes, the group range includes them.
    fn group(&mut self, i: &mut usize, limit: usize) -> usize {
        let open = self.toks[*i];
        *i += 1;
        let mut items = Vec::new();
        let mut end = open.to;
        loop {
            if *i >= limit {
                break;
            }
            if self.toks[*i].kind == Tok::Close {
                end = self.toks[*i].to;
                *i += 1;
                break;
            }
            let item = self.item(i, limit, true);
            match item {
                Some(id) => {
                    end = end.max(self.nodes[id].to);
                    items.push(id);
                }
                None => {
                    // a lone comma
                    if *i < limit && self.toks[*i].kind == Tok::Comma {
                        end = end.max(self.toks[*i].to);
                        *i += 1;
                    }
                }
            }
            if *i < limit && self.toks[*i].kind == Tok::Comma {
                end = end.max(self.toks[*i].to);
                *i += 1;
            }
        }
        self.node(open.from, end, items)
    }

    /// A run of atoms up to a comma (or, inside a group, the closing bracket).
    fn item(&mut self, i: &mut usize, limit: usize, in_group: bool) -> Option<usize> {
        let mut atoms = Vec::new();
        while *i < limit {
            let k = self.toks[*i].kind;
            if k == Tok::Comma || (in_group && k == Tok::Close) {
                break;
            }
            atoms.push(self.atom(i, limit));
        }
        match atoms.len() {
            0 => None,
            1 => Some(atoms[0]),
            _ => {
                let (from, to) = (self.nodes[atoms[0]].from, self.nodes[*atoms.last().unwrap()].to);
                Some(self.node(from, to, atoms))
            }
        }
    }

    /// Items separated by commas at the clause's own level.
    fn items(&mut self, i: &mut usize, limit: usize) -> Vec<usize> {
        let mut out = Vec::new();
        while *i < limit {
            if self.toks[*i].kind == Tok::Comma {
                *i += 1;
                continue;
            }
            match self.item_until_clause(i, limit) {
                Some(id) => out.push(id),
                None => break,
            }
        }
        out
    }

    /// Like `item`, but stops in front of a clause keyword as well.
    fn item_until_clause(&mut self, i: &mut usize, limit: usize) -> Option<usize> {
        let mut atoms = Vec::new();
        while *i < limit {
            let t = self.toks[*i];
            if t.kind == Tok::Comma || (t.clause && self.stops_item(*i)) {
                break;
            }
            atoms.push(self.atom(i, limit));
        }
        match atoms.len() {
            0 => None,
            1 => Some(atoms[0]),
            _ => {
                let (from, to) = (self.nodes[atoms[0]].from, self.nodes[*atoms.last().unwrap()].to);
                Some(self.node(from, to, atoms))
            }
        }
    }

    /// Does the clause keyword at `i` start a new clause? `by` after `group` / `order` does not count,
    /// and a keyword glued to a bracket is a function call (`values(`, `set(`) and stays an atom.
    fn stops_item(&self, i: usize) -> bool {
        !(self.toks.get(i + 1).is_some_and(|n| n.kind == Tok::Open && n.from == self.toks[i].to))
    }

    fn clause(&mut self, i: &mut usize, limit: usize) -> usize {
        let first = self.atom_simple(i);
        let mut last = first;
        // `group by`, `insert into`, `left outer join` ...: one keyword node for the whole run.
        while *i < limit && self.toks[*i].cont && self.stops_item(*i) {
            last = self.atom_simple(i);
        }
        let kw = if last == first {
            first
        } else {
            let (from, to) = (self.nodes[first].from, self.nodes[last].to);
            self.node(from, to, vec![])
        };
        let mut children = vec![kw];
        let items = self.items(i, limit);
        let to = items.last().map_or(self.nodes[kw].to, |&l| self.nodes[l].to);
        // The items form one node of their own, so `alt-a` on one of them selects only its siblings and
        // `alt-o` passes through "all the columns" on the way to the clause.
        match items.len() {
            0 => {}
            1 => children.push(items[0]),
            _ => {
                let (a, b) = (self.nodes[items[0]].from, self.nodes[*items.last().unwrap()].to);
                children.push(self.node(a, b, items));
            }
        }
        let from = self.nodes[kw].from;
        self.node(from, to, children)
    }

    fn atom_simple(&mut self, i: &mut usize) -> usize {
        let t = self.toks[*i];
        *i += 1;
        self.node(t.from, t.to, vec![])
    }

    fn statement(&mut self, i: &mut usize, limit: usize) -> Option<usize> {
        let mut clauses = Vec::new();
        while *i < limit {
            if self.toks[*i].clause {
                clauses.push(self.clause(i, limit));
            } else {
                // text before the first keyword, or a keyword we do not know
                let mut items = self.items(i, limit);
                if items.is_empty() {
                    break;
                }
                let (from, to) = (self.nodes[items[0]].from, self.nodes[*items.last().unwrap()].to);
                let id = if items.len() == 1 { items.remove(0) } else { self.node(from, to, items) };
                clauses.push(id);
            }
        }
        match clauses.len() {
            0 => None,
            1 => Some(clauses[0]),
            _ => {
                let (from, to) = (self.nodes[clauses[0]].from, self.nodes[*clauses.last().unwrap()].to);
                Some(self.node(from, to, clauses))
            }
        }
    }
}

impl Tree {
    pub(crate) fn build(t: Text) -> Tree {
        let chars: Vec<char> = t.chars().collect();
        let toks = lex(&chars);
        let mut b = Builder { toks: &toks, nodes: Vec::new() };
        let mut stmts = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            // a statement ends at the next `;` outside brackets
            let mut depth = 0usize;
            let mut end = i;
            while end < toks.len() {
                match toks[end].kind {
                    Tok::Open => depth += 1,
                    Tok::Close => depth = depth.saturating_sub(1),
                    Tok::Semi if depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            let mut j = i;
            if let Some(s) = b.statement(&mut j, end) {
                // The terminating `;` belongs to the statement.
                stmts.push(match toks.get(end) {
                    Some(semi) if semi.kind == Tok::Semi => {
                        let from = b.nodes[s].from;
                        b.node(from, semi.to, vec![s])
                    }
                    _ => s,
                });
            }
            // anything the statement parser left (a stray `)`) is skipped, so the loop always ends
            i = end + 1;
        }
        let (from, to) = match (stmts.first(), stmts.last()) {
            (Some(&a), Some(&z)) => (b.nodes[a].from, b.nodes[z].to),
            _ => (0, chars.len()),
        };
        let root = b.node(from, to, stmts);
        Tree { nodes: b.nodes, root }.folded()
    }

    /// Drop nodes that have exactly the range of their parent (a statement of one clause of one item).
    fn folded(mut self) -> Tree {
        for id in 0..self.nodes.len() {
            let Some(p) = self.nodes[id].parent else { continue };
            if (self.nodes[id].from, self.nodes[id].to) == (self.nodes[p].from, self.nodes[p].to) {
                // Adopt the children of the equal node and remove it from the parent.
                let kids = std::mem::take(&mut self.nodes[id].children);
                for &k in &kids {
                    self.nodes[k].parent = Some(p);
                }
                let pos = self.nodes[p].children.iter().position(|&c| c == id);
                if let Some(pos) = pos {
                    self.nodes[p].children.splice(pos..=pos, kids);
                }
                self.nodes[id].parent = None;
                self.nodes[id].from = usize::MAX;
                self.nodes[id].to = usize::MAX;
            }
        }
        self
    }

    fn span(&self, id: usize) -> Span {
        (self.nodes[id].from, self.nodes[id].to)
    }

    /// The deepest node containing `from..to`.
    fn descendant(&self, from: usize, to: usize) -> usize {
        let mut cur = self.root;
        'down: loop {
            for &c in &self.nodes[cur].children {
                let n = &self.nodes[c];
                if n.from <= from && to <= n.to {
                    cur = c;
                    continue 'down;
                }
            }
            return cur;
        }
    }

    /// `alt-o`: the smallest node that is strictly larger than `from..to`.
    pub(crate) fn expand(&self, from: usize, to: usize) -> Span {
        let mut node = self.descendant(from, to);
        while self.span(node) == (from, to) {
            match self.nodes[node].parent {
                Some(p) => node = p,
                None => break,
            }
        }
        self.span(node)
    }

    /// `alt-i` when there is no expansion to undo: the first child of the node around `from..to`.
    pub(crate) fn shrink(&self, from: usize, to: usize) -> Span {
        let node = self.descendant(from, to);
        match self.nodes[node].children.first() {
            Some(&c) => self.span(c),
            None => self.span(node),
        }
    }

    /// `alt-n` / `alt-p`: the next / previous sibling, climbing while there is none.
    pub(crate) fn sibling(&self, from: usize, to: usize, next: bool) -> Span {
        let mut node = self.descendant(from, to);
        loop {
            let Some(p) = self.nodes[node].parent else { return self.span(node) };
            let kids = &self.nodes[p].children;
            let pos = kids.iter().position(|&c| c == node).unwrap_or(0);
            let target = if next { kids.get(pos + 1) } else { pos.checked_sub(1).and_then(|q| kids.get(q)) };
            if let Some(&t) = target {
                return self.span(t);
            }
            node = p;
        }
    }

    /// `alt-a`: climb to a parent with more than one child and take all of them.
    pub(crate) fn all_siblings(&self, from: usize, to: usize) -> Vec<Span> {
        let mut node = self.descendant(from, to);
        loop {
            let Some(p) = self.nodes[node].parent else { return vec![(from, to)] };
            node = p;
            if self.nodes[node].children.len() > 1 {
                return self.nodes[node].children.iter().map(|&c| self.span(c)).collect();
            }
        }
    }

    /// `alt-I`: all children of the node around `from..to`.
    pub(crate) fn all_children(&self, from: usize, to: usize) -> Vec<Span> {
        let node = self.descendant(from, to);
        if self.nodes[node].children.is_empty() {
            vec![(from, to)]
        } else {
            self.nodes[node].children.iter().map(|&c| self.span(c)).collect()
        }
    }

    /// The node around `from..to`, for `alt-e` / `alt-b`.
    pub(crate) fn around(&self, from: usize, to: usize) -> Span {
        self.span(self.descendant(from, to))
    }

    /// For `alt-b` from the start of a node: the start of the nearest ancestor that starts earlier.
    pub(crate) fn parent_start(&self, from: usize, to: usize) -> Span {
        let mut node = self.descendant(from, to);
        let start = self.nodes[node].from;
        while self.nodes[node].from >= start {
            match self.nodes[node].parent {
                Some(p) => node = p,
                None => break,
            }
        }
        self.span(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn tree(s: &str) -> (Tree, Rope) {
        let r = Rope::from_str(s);
        (Tree::build(r.slice(..)), r)
    }

    fn text(r: &Rope, (a, b): Span) -> String {
        r.slice(a..b).to_string()
    }

    #[test]
    fn expand_climbs_word_call_clause_statement() {
        let src = "select a, abs(x + 1) from t where a > 2;\nselect 2;";
        let (t, r) = tree(src);
        let at = src.find("abs").unwrap();
        let mut span = (at, at + 1);
        let mut seen = Vec::new();
        for _ in 0..8 {
            span = t.expand(span.0, span.1);
            seen.push(text(&r, span));
        }
        assert_eq!(seen[0], "abs");
        assert_eq!(seen[1], "abs(x + 1)");
        assert_eq!(seen[2], "a, abs(x + 1)", "all the columns");
        assert_eq!(seen[3], "select a, abs(x + 1)");
        assert_eq!(seen[4], "select a, abs(x + 1) from t where a > 2");
        assert_eq!(seen[5], "select a, abs(x + 1) from t where a > 2;", "the semicolon belongs to the statement");
        assert_eq!(seen[6], src);
        assert_eq!(seen[7], src, "the root is the end of the line");
    }

    #[test]
    fn unfinished_statements_still_expand() {
        // The case from the bug report: nothing is closed.
        let src = "select a, abs(from time(";
        let (t, r) = tree(src);
        let at = src.find("abs").unwrap();
        let first = t.expand(at, at + 1);
        assert_eq!(text(&r, first), "abs");
        let second = t.expand(first.0, first.1);
        assert_eq!(text(&r, second), "abs(from time(");
        let third = t.expand(second.0, second.1);
        assert_eq!(text(&r, third), "a, abs(from time(");
        let fourth = t.expand(third.0, third.1);
        assert_eq!(text(&r, fourth), src);
    }

    #[test]
    fn siblings_and_children() {
        let src = "select a, b, c from t";
        let (t, r) = tree(src);
        let b = src.find('b').unwrap();
        assert_eq!(text(&r, t.sibling(b, b + 1, true)), "c");
        assert_eq!(text(&r, t.sibling(b, b + 1, false)), "a");
        let all: Vec<String> = t.all_siblings(b, b + 1).into_iter().map(|s| text(&r, s)).collect();
        assert_eq!(all, ["a", "b", "c"]);
        let a = src.find('a').unwrap();
        let kids: Vec<String> = t.all_children(a, a + 1).into_iter().map(|s| text(&r, s)).collect();
        assert_eq!(kids, ["a"], "a leaf is its own only child");
    }

    #[test]
    fn dotted_names_are_one_node() {
        let src = "select public.users.id from public.users";
        let (t, r) = tree(src);
        let at = src.find("users").unwrap();
        let word = t.expand(at, at + 1);
        assert_eq!(text(&r, word), "users");
        assert_eq!(text(&r, t.expand(word.0, word.1)), "public.users.id");
    }

    #[test]
    fn quotes_and_comments_do_not_confuse_the_scanner() {
        let src = "select 'a, b' , \"c d\" -- x, y\n from t";
        let (t, r) = tree(src);
        let at = src.find("'a").unwrap();
        assert_eq!(text(&r, t.expand(at + 1, at + 2)), "'a, b'");
        let empty = Tree::build(Rope::from_str("   ").slice(..));
        assert_eq!(empty.expand(0, 1), (0, 3));
    }
}
