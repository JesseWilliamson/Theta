//! Tree types: [`Kind`], [`Node`], [`ParseError`].

use crate::lexer::TokenKind;
use std::ops::Range;

/// What a node is. Token leaves carry their lexer kind; everything else is a syntax node.
///
/// The doc on each variant lists the children a consumer may rely on; anything not listed (keywords,
/// punctuation, trivia) is there only so that the children tile the span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A leaf. Trivia (whitespace, comments) are leaves too.
    Token(TokenKind),

    // ---- structure
    /// The whole input. Children: `Statement`, `BatchSeparator` and trivia.
    Root,
    /// One statement and its `;`. Exactly one statement node among its children (see below), or only the `;`.
    Statement,
    /// A client-side batch separator line such as T-SQL's `GO`.
    BatchSeparator,

    // ---- statements. Children listed are the *nodes*; keywords are tokens in between.
    /// `[WithClause] body [OrderBy] [Limit] [Offset] [Fetch] [Tail]`; body is `Select`, `Values`, `SetOp`,
    /// `ParenQuery` or `TableStmt`.
    Query,
    /// `WITH [RECURSIVE] Cte, ...`. On a DML statement it is the first child of that statement.
    WithClause,
    /// `name [(columns)] AS [MATERIALIZED] (Query | Insert | Update | Delete)`. First `Name`-like token is the
    /// CTE's name; `ColumnList` when it names its columns.
    Cte,
    /// `SELECT ...` up to (not including) `ORDER BY`/`LIMIT` and set operators.
    Select,
    /// `left UNION|INTERSECT|EXCEPT right`; two body children.
    SetOp,
    /// `VALUES (row), (row)`; each row is a `RowCtor`.
    Values,
    /// `( Query )` used as a query body.
    ParenQuery,
    /// PostgreSQL's `TABLE name`.
    TableStmt,
    SelectList,
    /// An item of a select list: `expr [AS alias]`, `*`, `t.*`. Children: expression nodes, `Alias`, `Star`.
    SelectItem,
    /// `[AS] name [(col, ...)]`. The name is the first non-keyword token; `ColumnList` for column aliases.
    Alias,
    /// `*` or `t.*` (the token `*` plus the qualifier tokens), also inside function arguments.
    Star,
    FromClause,
    /// `left join-keywords right [JoinConstraint]`.
    Join,
    /// `ON expr` or `USING (cols)`.
    JoinConstraint,
    /// A table (or view, CTE) reference: `Name [Alias]`.
    TableName,
    /// `(Query) [Alias]`, `LATERAL (Query) [Alias]`.
    DerivedTable,
    /// A set-returning function used as a table: `FuncCall [Alias]`.
    TableFunc,
    /// A parenthesised join: `( table_ref )`.
    ParenFrom,
    WhereClause,
    GroupBy,
    Having,
    /// `WINDOW name AS (...)`.
    WindowClause,
    /// `( [base] [PARTITION BY ..] [ORDER BY ..] [frame] )`.
    WindowDef,
    Qualify,
    OrderBy,
    /// One sort key: `expr [ASC|DESC] [NULLS FIRST|LAST]`.
    OrderItem,
    /// `LIMIT`/`TOP`/`OFFSET`/`FETCH` and their operands.
    Limit,
    /// Everything after the query that is dialect specific (`FOR UPDATE`, `OPTION (...)`, `SETTINGS ...`, `INTO OUTFILE`).
    Tail,
    /// `INTO target` of `SELECT ... INTO`.
    IntoClause,

    Insert,
    Update,
    Delete,
    Merge,
    /// `WHEN [NOT] MATCHED ... THEN action` of a MERGE.
    MergeClause,
    /// `(a, b, c)`: only identifiers and commas.
    ColumnList,
    /// `target = expr` in SET / ON CONFLICT DO UPDATE / ON DUPLICATE KEY UPDATE.
    Assignment,
    SetClause,
    /// `ON CONFLICT ...`.
    OnConflict,
    /// `ON DUPLICATE KEY UPDATE ...`.
    OnDuplicate,
    /// `RETURNING` (PostgreSQL, SQLite, MariaDB, DuckDB) or `OUTPUT` (T-SQL): a select list.
    Returning,
    /// `USING` of DELETE, `FROM` of UPDATE: a from list.
    UsingClause,

    /// `EXPLAIN [options] statement`. Child: the explained statement node.
    Explain,
    /// CREATE / ALTER / DROP / TRUNCATE / RENAME. `Name` children are the object defined (first) and others
    /// referenced; `Query` when it holds one (`CREATE VIEW ... AS`).
    Ddl,
    /// One `name type ...` entry of a CREATE TABLE list.
    ColumnDef,
    /// The parenthesised element list of CREATE TABLE (`ColumnDef`s and constraints).
    ColumnDefList,
    /// Any statement the parser knows only by its first word.
    Other,

    // ---- expressions
    /// A dotted name: identifier tokens and `.`.
    Name,
    /// A possibly qualified column reference (`a`, `t.a`, `s.t.a`): identifier tokens and `.`.
    ColumnRef,
    /// A number, string, `NULL`, `TRUE`, `FALSE`, typed literal.
    Literal,
    /// `$1`, `?`, `:name`, `@name`.
    Param,
    /// `Name ArgList [Filter] [Over] [WithinGroup]`.
    FuncCall,
    /// `( args )`: expressions, `Star`, `Query`, keywords.
    ArgList,
    /// `FILTER (WHERE ...)`.
    Filter,
    /// `OVER name` or `OVER ( window )`.
    Over,
    /// `WITHIN GROUP (ORDER BY ...)`.
    WithinGroup,
    /// `left op right` (including AND, OR, comparison, LIKE, IN, BETWEEN, IS, `->`): `left op right`.
    Binary,
    /// `op operand` (`NOT`, `-`, custom prefix operators).
    Prefix,
    /// `operand ISNULL`, `operand NOT NULL`, `operand COLLATE x`, `operand AT TIME ZONE x`.
    Postfix,
    /// `expr :: type`, `CAST(expr AS type)` is a `FuncCall`.
    Cast,
    /// A type name: tokens, with an optional `( args )` and `[]` suffix.
    TypeName,
    Case,
    /// `WHEN cond THEN result`.
    CaseWhen,
    /// `( expr )`.
    Paren,
    /// `( Query )` in expression position.
    Subquery,
    /// `EXISTS ( Query )`.
    Exists,
    /// `( a, b, c )` and `ROW(...)`, `VALUES` rows.
    RowCtor,
    /// `ARRAY[...]`, `[...]`, `{...}`.
    ArrayCtor,
    /// `expr [ index ]`.
    Index,
    /// `expr . field`, `expr . *`.
    FieldAccess,
    /// `INTERVAL '1' DAY`.
    Interval,
    /// Tokens the grammar does not understand. Never an error by itself.
    Unknown,
}

impl Kind {
    pub fn is_token(self) -> bool {
        matches!(self, Kind::Token(_))
    }

    /// Whether this node is an expression the resolver should look inside.
    pub fn is_expr(self) -> bool {
        matches!(
            self,
            Kind::ColumnRef
                | Kind::Literal
                | Kind::Param
                | Kind::FuncCall
                | Kind::Binary
                | Kind::Prefix
                | Kind::Postfix
                | Kind::Cast
                | Kind::Case
                | Kind::Paren
                | Kind::Subquery
                | Kind::Exists
                | Kind::RowCtor
                | Kind::ArrayCtor
                | Kind::Index
                | Kind::FieldAccess
                | Kind::Interval
        )
    }
}

/// A syntax node or token leaf with its byte span. `children` is empty for leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub kind: Kind,
    pub span: Range<usize>,
    pub children: Vec<Node>,
}

/// Dropping a deep tree must not recurse: every dialect's parser is fed pasted text of any shape.
impl Drop for Node {
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.children);
        while let Some(mut n) = stack.pop() {
            stack.append(&mut n.children);
        }
    }
}

impl Node {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.span.clone()]
    }

    pub fn is_trivia(&self) -> bool {
        matches!(
            self.kind,
            Kind::Token(TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment)
        )
    }

    /// Child *nodes* (not tokens).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|c| !c.kind.is_token())
    }

    /// Child tokens that are not trivia.
    pub fn tokens(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|c| c.kind.is_token() && !c.is_trivia())
    }

    pub fn child(&self, kind: Kind) -> Option<&Node> {
        self.children.iter().find(|c| c.kind == kind)
    }

    pub fn children_of(&self, kind: Kind) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(move |c| c.kind == kind)
    }

    /// Is this a token leaf that is the bare word `kw` (case-insensitive)?
    pub fn is_kw(&self, sql: &str, kw: &str) -> bool {
        self.kind == Kind::Token(TokenKind::Word) && self.text(sql).eq_ignore_ascii_case(kw)
    }

    /// Does this node have a direct token child that is the keyword `kw`?
    pub fn has_kw(&self, sql: &str, kw: &str) -> bool {
        self.children.iter().any(|c| c.is_kw(sql, kw))
    }

    /// Identifier-like token leaves of a `Name` / `ColumnRef` (words and quoted identifiers), in order.
    pub fn ident_tokens(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|c| {
            matches!(c.kind, Kind::Token(TokenKind::Word | TokenKind::QuotedIdent | TokenKind::String))
        })
    }

    /// The deepest node whose span contains `offset` (a cursor between two bytes counts as inside a node
    /// that ends at it only when no other node starts there). Returns the path from the root, outermost first.
    pub fn path_at(&self, offset: usize) -> Vec<&Node> {
        let mut path = vec![self];
        let mut cur = self;
        loop {
            // A node whose span contains the offset; for a boundary shared by two nodes prefer the one
            // that starts at it, as a cursor sits "before" the character it is on.
            let next =
                cur.children.iter().find(|c| c.span.start <= offset && offset < c.span.end).or_else(|| {
                    cur.children.iter().rev().find(|c| c.span.end == offset && !c.span.is_empty())
                });
            match next {
                Some(c) => {
                    path.push(c);
                    cur = c;
                }
                None => break,
            }
        }
        path
    }

    /// Visit every node (pre-order), tokens included.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Node)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    /// Whether every node's children tile its span and leaves concatenate to the input. Used by tests and
    /// debug assertions; cheap enough to run on every parse in tests.
    pub fn check_tiling(&self, sql: &str) -> Result<(), String> {
        fn go(n: &Node, sql: &str) -> Result<(), String> {
            if n.kind.is_token() {
                return if n.children.is_empty() { Ok(()) } else { Err("token with children".into()) };
            }
            let mut at = n.span.start;
            for c in &n.children {
                if c.span.start != at {
                    return Err(format!(
                        "gap or overlap in {:?} at {at}: child {:?} starts at {}",
                        n.kind, c.kind, c.span.start
                    ));
                }
                go(c, sql)?;
                at = c.span.end;
            }
            if at != n.span.end {
                return Err(format!("{:?} ends at {} but children end at {at}", n.kind, n.span.end));
            }
            if !sql.is_char_boundary(n.span.start) || !sql.is_char_boundary(n.span.end) {
                return Err(format!("{:?} span not on char boundary", n.kind));
            }
            Ok(())
        }
        if self.span != (0..sql.len()) {
            return Err(format!("root span {:?} != 0..{}", self.span, sql.len()));
        }
        go(self, sql)
    }
}

/// A syntax error the parser is certain about. `incomplete` is true when the only thing wrong is that
/// the statement stops too early; an editor should hold such errors back while the user is still typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub span: Range<usize>,
    pub message: String,
    pub incomplete: bool,
}
