//! Rename of the names a script introduces: table aliases, CTEs, the columns a CTE or a select list
//! exposes. All references in scope change in one edit set. A rename that would collide with, or be captured
//! by, another name is refused with the reason; so is renaming something that is not the script's own (a table
//! or column of the database is renamed in the table editor, not here).

use super::Edit;
use super::navigate::{Target, target_at, usages_of};
use crate::analysis::{Analysis, ScopeKind, SymbolId, SymbolKind};
use crate::keywords::quote_if_needed;
use crate::syntax::{Kind, Node};
use theta_core::DatabaseKind;

const OUT_OF_SCOPE: &str =
    "Tables and columns of the database are renamed in the table editor (Modify Table), not from the editor";

/// What can be renamed at `offset`: the current name and the symbol, or the reason it cannot.
pub fn rename_prepare(a: &Analysis, offset: usize) -> Result<(String, SymbolId), String> {
    let Some((target, _)) = target_at(a, offset) else {
        return Err("Put the cursor on an alias, a CTE or a column alias to rename it".to_string());
    };
    match target {
        Target::Table(t) => Err(format!("'{}' is a table of the database. {OUT_OF_SCOPE}", t.name)),
        Target::Column { column, .. } => Err(format!("'{column}' is a column of a table. {OUT_OF_SCOPE}")),
        Target::Function(f) => {
            Err(format!("'{f}' is a function; functions cannot be renamed from the editor"))
        }
        Target::Symbol(s) => {
            let sym = &a.symbols[s];
            if !sym.renamable {
                return Err(format!(
                    "'{}' is a column that passes through from a table; rename its alias instead, or the column in the table editor",
                    sym.name
                ));
            }
            Ok((sym.name.clone(), s))
        }
    }
}

/// The edits that rename the symbol at `offset` to `new_name` (given as typed: a bare identifier, or quoted).
pub fn rename_at(
    a: &Analysis,
    sql: &str,
    kind: DatabaseKind,
    offset: usize,
    new_name: &str,
) -> Result<Vec<Edit>, String> {
    let (old, sym_id) = rename_prepare(a, offset)?;
    let typed = new_name.trim();
    if typed.is_empty() {
        return Err("The new name is empty".to_string());
    }
    let (bare, user_quoted) = unquote(typed);
    if bare.is_empty() {
        return Err("The new name is empty".to_string());
    }
    if bare == old && !user_quoted {
        return Err("The name is unchanged".to_string());
    }
    if !user_quoted && !typed.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
        return Err(format!("'{typed}' is not a plain identifier; put it in quotes to use it as a name"));
    }
    let text = if user_quoted { typed.to_string() } else { quote_if_needed(&bare, kind) };
    let sym = &a.symbols[sym_id];
    let same = |x: &str| x.eq_ignore_ascii_case(&bare);
    let scope = &a.scopes[sym.scope];
    match sym.kind {
        SymbolKind::TableAlias => {
            if let Some(other) = scope.sources.iter().find(|s| s.symbol != Some(sym_id) && same(&s.name)) {
                return Err(format!(
                    "'{bare}' is already used in this FROM clause by {}",
                    describe_source(&other.name)
                ));
            }
            // an inner query that declares the same name would capture references to the outer alias
            for (i, inner) in a.scopes.iter().enumerate() {
                if i != sym.scope
                    && is_inside(&inner.span, &scope.span)
                    && inner.sources.iter().any(|s| same(&s.name))
                    && uses_reach_inner(a, sym_id, &inner.span)
                {
                    return Err(format!(
                        "'{bare}' is the name of a table or alias in a subquery that uses '{old}': the references would be captured"
                    ));
                }
            }
        }
        SymbolKind::Cte => {
            if scope.ctes.iter().any(|&c| c != sym_id && same(&a.symbols[c].name)) {
                return Err(format!("There is already a CTE called '{bare}' in this WITH clause"));
            }
            // A table of the database with the new name, used where the CTE is visible, would be hidden by it.
            let region = enclosing_statement(a, scope.span.start);
            for r in &a.refs {
                if is_inside(&r.span, &region)
                    && matches!(&r.resolution, crate::analysis::Resolution::Table(t) if same(&t.name))
                {
                    return Err(format!(
                        "'{bare}' is the name of a table used in this statement; the CTE would hide it"
                    ));
                }
            }
        }
        SymbolKind::OutputColumn => {
            if scope.outputs.iter().any(|&o| o != sym_id && same(&a.symbols[o].name)) {
                return Err(format!("This select list already has a column called '{bare}'"));
            }
            // ORDER BY / GROUP BY prefer an output name: a bare reference to a table column with the new
            // name would silently change meaning.
            let spans = order_group_spans(a, &scope.span);
            for r in &a.refs {
                if matches!(&r.resolution, crate::analysis::Resolution::Column { column, .. } if same(column))
                    && r.scope == sym.scope
                    && spans.iter().any(|s| is_inside(&r.span, s))
                    && !preceded_by_dot(sql, r.span.start)
                {
                    return Err(format!(
                        "'{bare}' is also a column used unqualified in ORDER BY or GROUP BY of this query; it would refer to the alias afterwards"
                    ));
                }
            }
        }
        SymbolKind::ColumnAlias => {
            if scope.kind == ScopeKind::Empty {
                return Err("This name cannot be renamed here".to_string());
            }
        }
    }
    let usages = usages_of(a, &Target::Symbol(sym_id));
    Ok(usages.into_iter().map(|u| Edit::replace(u.range, text.clone())).collect())
}

fn unquote(s: &str) -> (String, bool) {
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"')
            || (b[0] == b'`' && b[b.len() - 1] == b'`')
            || (b[0] == b'[' && b[b.len() - 1] == b']'))
    {
        (s[1..s.len() - 1].replace("\"\"", "\""), true)
    } else {
        (s.to_string(), false)
    }
}

fn describe_source(name: &str) -> String {
    format!("'{name}'")
}

fn is_inside(inner: &std::ops::Range<usize>, outer: &std::ops::Range<usize>) -> bool {
    inner.start >= outer.start && inner.end <= outer.end
}

fn preceded_by_dot(sql: &str, at: usize) -> bool {
    sql[..at].trim_end().ends_with('.')
}

/// Does some use of the symbol sit inside `inner` (so that an alias of the same name declared there would
/// capture it)?
fn uses_reach_inner(a: &Analysis, sym: SymbolId, inner: &std::ops::Range<usize>) -> bool {
    a.refs_to_symbol(sym).any(|r| is_inside(&r.span, inner))
}

/// The statement text range that contains `at`.
fn enclosing_statement(a: &Analysis, at: usize) -> std::ops::Range<usize> {
    for s in a.parse.statements() {
        if s.span.start <= at && at <= s.span.end {
            return s.span.clone();
        }
    }
    0..usize::MAX
}

/// The spans of ORDER BY and GROUP BY clauses of the select with span `select`, including the ORDER BY of the
/// enclosing query.
fn order_group_spans(a: &Analysis, select: &std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
    fn walk(n: &Node, select: &std::ops::Range<usize>, out: &mut Vec<std::ops::Range<usize>>) {
        if n.kind == Kind::Query && n.children.iter().any(|c| c.kind == Kind::Select && &c.span == select) {
            for c in n.nodes() {
                if c.kind == Kind::OrderBy {
                    out.push(c.span.clone());
                }
            }
        }
        if n.kind == Kind::Select && &n.span == select {
            for c in n.nodes() {
                if matches!(c.kind, Kind::GroupBy | Kind::Having) {
                    out.push(c.span.clone());
                }
            }
        }
        for c in n.nodes() {
            walk(c, select, out);
        }
    }
    let mut out = Vec::new();
    walk(&a.parse.root, select, &mut out);
    out
}
