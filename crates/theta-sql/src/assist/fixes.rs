//! Quick fixes (for a diagnostic at the cursor) and refactorings (for the construct at the cursor).

use super::Edit;
use crate::analysis::{Analysis, DiagKind, Resolution, ScopeId, Source, SourceKind, edit_distance};
use crate::keywords::{is_format_keyword, is_keyword_any, quote_if_needed};
use crate::lexer::TokenKind;
use crate::syntax::{Kind, Node};
use std::ops::Range;
use theta_core::{Catalog, DatabaseKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixKind {
    /// Repairs a reported problem.
    QuickFix,
    /// Restructures valid SQL without changing what it does.
    Refactor,
    /// Pure formatting of the statement.
    Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub kind: FixKind,
    pub edits: Vec<Edit>,
}

/// The actions available at the byte offset `at` (the start of the selection). Quick fixes for the diagnostics
/// there come first, in the order of likelihood.
pub fn code_actions(
    sql: &str,
    kind: DatabaseKind,
    _catalog: Option<&Catalog>,
    a: &Analysis,
    at: usize,
) -> Vec<Fix> {
    let mut out: Vec<Fix> = Vec::new();
    // A cursor right after a word belongs to that word, as it does for the user.
    let at_word_end = at > 0
        && sql.as_bytes().get(at - 1).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        && sql.as_bytes().get(at).is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_'));
    let probe = if at_word_end { at - 1 } else { at };
    let path: Vec<&Node> = a.parse.root.path_at(probe).into_iter().filter(|n| !n.kind.is_token()).collect();
    // The problems under the cursor; when there are none, those of the statement (a fix is useful from
    // anywhere in the statement, and the title names the word it changes).
    let here = |d: &&crate::analysis::Diag| d.range.start <= at && at <= d.range.end;
    let relevant: Vec<&crate::analysis::Diag> = if a.diagnostics.iter().any(|d| here(&d)) {
        a.diagnostics.iter().filter(here).collect()
    } else {
        a.diagnostics.iter().collect()
    };
    for d in relevant {
        match &d.kind {
            DiagKind::UnknownTable { name, candidates }
            | DiagKind::UnknownColumn { name, candidates, .. }
            | DiagKind::UnknownQualifier { name, candidates } => {
                let last = name.rsplit('.').next().unwrap_or(name);
                for c in suggestions(last, candidates) {
                    out.push(Fix {
                        title: format!("Change '{}' to '{c}'", &sql[d.range.clone()]),
                        kind: FixKind::QuickFix,
                        edits: vec![Edit::replace(d.range.clone(), quote_if_needed(&c, kind))],
                    });
                }
            }
            DiagKind::AmbiguousColumn { name, qualifiers } => {
                for q in qualifiers {
                    out.push(Fix {
                        title: format!("Qualify as '{q}.{name}'"),
                        kind: FixKind::QuickFix,
                        edits: vec![Edit::insert(d.range.start, format!("{}.", quote_if_needed(q, kind)))],
                    });
                }
            }
            DiagKind::NotGrouped { .. } => {
                if let Some(f) = add_group_by(a, sql, d.range.start) {
                    out.push(f);
                }
            }
            _ => {}
        }
    }
    out.extend(expand_star(a, kind, probe));
    out.extend(add_alias(a, sql, kind, &path));
    out.extend(comma_join_to_join(a, sql, &path));
    out.extend(flip_join(sql, &path));
    out.extend(cte_actions(a, sql, kind, &path, probe));
    out.extend(keyword_case(a, sql, &path));
    out
}

// ---- did you mean

/// Candidates close to `name`: within an edit distance that grows with its length, or starting with it.
pub(crate) fn suggestions(name: &str, candidates: &[String]) -> Vec<String> {
    let max = (name.chars().count() / 3).clamp(1, 3);
    let lower = name.to_lowercase();
    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .filter(|c| !c.eq_ignore_ascii_case(name))
        .filter_map(|c| {
            let d = edit_distance(name, c);
            let prefix = lower.len() >= 3 && c.to_lowercase().starts_with(&lower);
            if d <= max {
                Some((d, c))
            } else if prefix {
                Some((max + 1, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(d, _)| *d);
    let mut out: Vec<String> = Vec::new();
    for (_, c) in scored {
        if !out.contains(c) {
            out.push(c.clone());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

// ---- expand *

fn expand_star(a: &Analysis, kind: DatabaseKind, at: usize) -> Option<Fix> {
    let s = a
        .stars
        .iter()
        .find(|s| s.complete && s.span.start <= at && at <= s.span.end && !s.columns.is_empty())?;
    let multi = a.scopes[s.scope].sources.iter().filter(|x| x.kind != SourceKind::Pseudo).count() > 1;
    let list: Vec<String> = s
        .columns
        .iter()
        .map(|(q, c)| {
            if multi || s.qualified {
                format!("{}.{}", quote_if_needed(q, kind), quote_if_needed(c, kind))
            } else {
                quote_if_needed(c, kind)
            }
        })
        .collect();
    Some(Fix {
        title: "Expand * into the column list".to_string(),
        kind: FixKind::Refactor,
        edits: vec![Edit::replace(s.span.clone(), list.join(", "))],
    })
}

// ---- add table alias

fn make_alias(table: &str, taken: &[String], kind: DatabaseKind) -> String {
    let mut initials: String = table
        .split(['_', ' ', '-'])
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.chars().next())
        .collect::<String>()
        .to_lowercase();
    if initials.is_empty() {
        initials = "t".to_string();
    }
    let is_free = |c: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(c)) && !is_keyword_any(c, kind);
    if is_free(&initials) {
        return initials;
    }
    for n in 1.. {
        let cand = format!("{initials}{n}");
        if is_free(&cand) {
            return cand;
        }
    }
    unreachable!()
}

/// Is a reference made in scope `from` a reference to the source of scope `sid` (and not hidden by a source of
/// the same table in a scope in between)?
fn reaches(a: &Analysis, from: ScopeId, sid: ScopeId, src: &Source) -> bool {
    let mut cur = Some(from);
    while let Some(s) = cur {
        if s == sid {
            return true;
        }
        if a.scopes[s].sources.iter().any(|x| x.table == src.table && x.table.is_some()) {
            return false;
        }
        cur = a.scopes[s].parent;
    }
    false
}

fn preceded_by_dot(sql: &str, at: usize) -> bool {
    sql[..at].trim_end().ends_with('.')
}

fn add_alias(a: &Analysis, sql: &str, kind: DatabaseKind, path: &[&Node]) -> Option<Fix> {
    let tn = path.iter().rev().find(|n| n.kind == Kind::TableName)?;
    if tn.child(Kind::Alias).is_some() || !path.iter().any(|n| n.kind == Kind::FromClause) {
        return None;
    }
    let (sid, src) = a.scopes.iter().enumerate().find_map(|(i, s)| {
        s.sources.iter().find(|x| x.span == tn.span && x.kind == SourceKind::Table).map(|x| (i, x))
    })?;
    let table = src.table.clone()?;
    if table.script {
        return None;
    }
    let taken: Vec<String> = a.scopes[sid].sources.iter().map(|s| s.name.clone()).collect();
    let alias = make_alias(&table.name, &taken, kind);
    let name_node = tn.child(Kind::Name)?;
    let mut edits = vec![Edit::insert(name_node.span.end, format!(" {alias}"))];
    for r in &a.refs {
        if !reaches(a, r.scope, sid, src) || tn.span.contains(&r.span.start) {
            continue;
        }
        match (&r.resolution, r.role) {
            (Resolution::Table(t), crate::analysis::RefRole::Qualifier) if *t == table => {
                edits.push(Edit::replace(r.span.clone(), alias.clone()));
            }
            (Resolution::Column { table: t, .. }, crate::analysis::RefRole::Column)
                if *t == table && !preceded_by_dot(sql, r.span.start) =>
            {
                edits.push(Edit::insert(r.span.start, format!("{alias}.")));
            }
            _ => {}
        }
    }
    Some(Fix {
        title: format!("Add alias '{alias}' to {} and qualify its columns", table.name),
        kind: FixKind::Refactor,
        edits,
    })
}

// ---- GROUP BY

fn add_group_by(a: &Analysis, sql: &str, at: usize) -> Option<Fix> {
    let path = a.parse.root.path_at(at);
    let select = path.iter().rev().find(|n| n.kind == Kind::Select)?;
    // every ungrouped column of this very select
    let mut cols: Vec<String> = Vec::new();
    for d in &a.diagnostics {
        if !matches!(d.kind, DiagKind::NotGrouped { .. })
            || !(select.span.start <= d.range.start && d.range.end <= select.span.end)
        {
            continue;
        }
        let p = a.parse.root.path_at(d.range.start);
        // not one that belongs to a nested select
        if p.iter().rev().find(|n| n.kind == Kind::Select).map(|n| &n.span) != Some(&select.span) {
            continue;
        }
        let text = p
            .iter()
            .rev()
            .find(|n| n.kind == Kind::ColumnRef)
            .map(|n| n.text(sql).to_string())
            .unwrap_or_else(|| sql[d.range.clone()].to_string());
        if !cols.iter().any(|c| c.eq_ignore_ascii_case(&text)) {
            cols.push(text);
        }
    }
    if cols.is_empty() {
        return None;
    }
    let list = cols.join(", ");
    let title = format!("Add {list} to GROUP BY");
    let edit = match select.child(Kind::GroupBy) {
        Some(g) => {
            let last = g.nodes().filter(|n| n.kind != Kind::Unknown).last()?;
            Edit::insert(last.span.end, format!(", {list}"))
        }
        None => {
            let last =
                select.nodes().filter(|n| matches!(n.kind, Kind::FromClause | Kind::WhereClause)).last()?;
            Edit::insert(last.span.end, format!(" GROUP BY {list}"))
        }
    };
    Some(Fix { title, kind: FixKind::QuickFix, edits: vec![edit] })
}

// ---- comma join -> JOIN

/// The indexes of the FROM items of scope `sid` that the references in `span` point at; `None` when some
/// reference cannot be attributed to one source.
fn items_referenced(a: &Analysis, sid: ScopeId, span: &Range<usize>) -> Option<Vec<usize>> {
    let sources = &a.scopes[sid].sources;
    let mut set: Vec<usize> = Vec::new();
    for r in a.refs.iter().filter(|r| span.start <= r.span.start && r.span.end <= span.end) {
        let idx = match &r.resolution {
            Resolution::Symbol(s) => sources.iter().position(|x| x.symbol == Some(*s)),
            Resolution::Table(t) | Resolution::Column { table: t, .. } => {
                let hits: Vec<usize> = sources
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.table.as_ref() == Some(t))
                    .map(|(i, _)| i)
                    .collect();
                if hits.len() == 1 { Some(hits[0]) } else { None }
            }
            _ => continue,
        };
        // a reference to something outside this scope is not about the join
        if let Some(i) = idx {
            if !set.contains(&i) {
                set.push(i);
            }
        } else if matches!(r.resolution, Resolution::Column { .. } | Resolution::Table(_)) && r.scope == sid {
            return None;
        }
    }
    set.sort_unstable();
    Some(set)
}

fn comma_join_to_join(a: &Analysis, sql: &str, path: &[&Node]) -> Option<Fix> {
    let from = path.iter().rev().find(|n| n.kind == Kind::FromClause)?;
    let select = path.iter().rev().find(|n| n.kind == Kind::Select)?;
    let items: Vec<&Node> = from.nodes().filter(|n| n.kind != Kind::Unknown).collect();
    if items.len() < 2 || !items.iter().all(|n| matches!(n.kind, Kind::TableName | Kind::DerivedTable)) {
        return None;
    }
    if items.iter().any(|n| n.has_kw(sql, "LATERAL")) {
        return None;
    }
    let (sid, scope) =
        a.scopes.iter().enumerate().find(|(_, s)| s.span == select.span && s.sources.len() == items.len())?;
    let _ = scope;
    // The WHERE conjuncts, when the WHERE follows the FROM directly.
    let where_node = select.child(Kind::WhereClause);
    let mut conjuncts: Vec<&Node> = Vec::new();
    if let Some(w) = where_node {
        let adjacent =
            select.nodes().skip_while(|n| n.span != from.span).nth(1).is_some_and(|n| n.span == w.span);
        if adjacent && let Some(e) = w.nodes().next() {
            let and_chain = e.kind == Kind::Binary && e.tokens().all(|t| t.is_kw(sql, "AND"));
            if and_chain {
                conjuncts.extend(e.nodes());
            } else {
                conjuncts.push(e);
            }
        }
    }
    let refs: Vec<Option<Vec<usize>>> = conjuncts.iter().map(|c| items_referenced(a, sid, &c.span)).collect();
    let mut used = vec![false; conjuncts.len()];
    let mut text = items[0].text(sql).to_string();
    for (k, item) in items.iter().enumerate().skip(1) {
        let conds: Vec<usize> = (0..conjuncts.len())
            .filter(|&i| {
                // a join condition links the new item to an earlier one; a filter on one table stays in WHERE
                !used[i]
                    && refs[i]
                        .as_ref()
                        .is_some_and(|r| r.contains(&k) && r.len() >= 2 && r.iter().all(|&x| x <= k))
            })
            .collect();
        if conds.is_empty() {
            text.push_str(&format!("\nCROSS JOIN {}", item.text(sql)));
        } else {
            for &i in &conds {
                used[i] = true;
            }
            let on: Vec<&str> = conds.iter().map(|&i| conjuncts[i].text(sql)).collect();
            text.push_str(&format!("\nJOIN {} ON {}", item.text(sql), on.join(" AND ")));
        }
    }
    let mut edits = vec![Edit::replace(items[0].span.start..items[items.len() - 1].span.end, text)];
    if let Some(w) = where_node
        && !conjuncts.is_empty()
        && used.iter().any(|&u| u)
    {
        let rest: Vec<&str> =
            conjuncts.iter().zip(&used).filter(|(_, u)| !**u).map(|(c, _)| c.text(sql)).collect();
        if rest.is_empty() {
            // remove the whole WHERE, with the whitespace in front of it
            let start = sql[..w.span.start].trim_end().len().max(from.span.end);
            edits.push(Edit::delete(start..w.span.end));
        } else {
            edits.push(Edit::replace(w.span.clone(), format!("WHERE {}", rest.join(" AND "))));
        }
    }
    Some(Fix {
        title: "Convert the comma join to explicit JOINs".to_string(),
        kind: FixKind::Refactor,
        edits,
    })
}

// ---- flip join

fn flip_join(sql: &str, path: &[&Node]) -> Option<Fix> {
    let join = path.iter().rev().find(|n| n.kind == Kind::Join)?;
    let factors: Vec<&Node> = join
        .nodes()
        .filter(|n| {
            matches!(
                n.kind,
                Kind::TableName | Kind::DerivedTable | Kind::TableFunc | Kind::ParenFrom | Kind::Join
            )
        })
        .collect();
    let constraints = join.children_of(Kind::JoinConstraint).count();
    if factors.len() != 2
        || constraints > 1
        || factors.iter().any(|f| f.kind == Kind::Join || f.has_kw(sql, "LATERAL"))
    {
        return None;
    }
    let (f1, f2) = (factors[0], factors[1]);
    let mods = &sql[f1.span.end..f2.span.start];
    if mods.to_ascii_uppercase().contains("APPLY") || mods.to_ascii_uppercase().contains("STRAIGHT_JOIN") {
        return None;
    }
    // swap LEFT and RIGHT, keep everything else (INNER, CROSS, FULL, NATURAL, OUTER) as written
    let mut flipped = String::new();
    let mut word = String::new();
    let flush = |w: &mut String, out: &mut String| {
        if w.is_empty() {
            return;
        }
        let up = w.to_ascii_uppercase();
        let swapped = match up.as_str() {
            "LEFT" => Some("RIGHT"),
            "RIGHT" => Some("LEFT"),
            _ => None,
        };
        match swapped {
            Some(s) if w.chars().all(|c| c.is_ascii_lowercase()) => out.push_str(&s.to_ascii_lowercase()),
            Some(s) => out.push_str(s),
            None => out.push_str(w),
        }
        w.clear();
    };
    for c in mods.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut flipped);
            flipped.push(c);
        }
    }
    flush(&mut word, &mut flipped);
    let tail = &sql[f2.span.end..join.span.end];
    let new = format!("{}{}{}{}", f2.text(sql), flipped, f1.text(sql), tail);
    Some(Fix {
        title: "Flip the sides of the JOIN".to_string(),
        kind: FixKind::Refactor,
        edits: vec![Edit::replace(join.span.clone(), new)],
    })
}

// ---- CTEs

fn cte_actions(a: &Analysis, sql: &str, kind: DatabaseKind, path: &[&Node], at: usize) -> Vec<Fix> {
    let mut out = Vec::new();
    if let Some(f) = introduce_cte(a, sql, kind, path) {
        out.push(f);
    }
    if let Some(f) = inline_cte(a, sql, at) {
        out.push(f);
    }
    out
}

fn introduce_cte(a: &Analysis, sql: &str, kind: DatabaseKind, path: &[&Node]) -> Option<Fix> {
    let derived = path.iter().rev().find(|n| n.kind == Kind::DerivedTable)?;
    if derived.has_kw(sql, "LATERAL") {
        return None;
    }
    let alias_node = derived.child(Kind::Alias)?;
    if alias_node.child(Kind::ColumnList).is_some() {
        return None;
    }
    let alias = alias_node.tokens().find(|t| !t.is_kw(sql, "AS"))?;
    let alias_text = alias.text(sql).to_string();
    let query = derived.child(Kind::Query)?;
    // The CTE goes on the statement's top query; INSERT, UPDATE and friends are not handled.
    let top = path.iter().find(|n| n.kind != Kind::Root && n.kind != Kind::Statement)?;
    if top.kind != Kind::Query {
        return None;
    }
    // A derived table that reads the surrounding query's aliases cannot move out.
    let inside = |r: &Range<usize>| derived.span.start <= r.start && r.end <= derived.span.end;
    for r in a.refs.iter().filter(|r| inside(&r.span)) {
        if let Resolution::Symbol(s) = r.resolution {
            let sym = &a.symbols[s];
            if matches!(sym.kind, crate::analysis::SymbolKind::TableAlias) && !inside(&sym.span) {
                return None;
            }
        }
    }
    // the name must be new in this statement
    let alias_canon = alias_text.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']').to_string();
    if a.symbols
        .iter()
        .any(|s| s.kind == crate::analysis::SymbolKind::Cte && s.name.eq_ignore_ascii_case(&alias_canon))
        || a.refs.iter().any(
            |r| matches!(&r.resolution, Resolution::Table(t) if t.name.eq_ignore_ascii_case(&alias_canon)),
        )
    {
        return None;
    }
    let body = query.text(sql);
    let def = format!("{} AS ({body})", quote_if_needed(&alias_canon, kind));
    let mut edits = Vec::new();
    match top.child(Kind::WithClause) {
        Some(w) => {
            let last = w.children_of(Kind::Cte).last()?;
            edits.push(Edit::insert(last.span.end, format!(",\n{def}")));
        }
        None => edits.push(Edit::insert(top.span.start, format!("WITH {def}\n"))),
    }
    edits.push(Edit::replace(derived.span.clone(), alias_text));
    Some(Fix {
        title: format!("Extract the subquery '{alias_canon}' into a CTE"),
        kind: FixKind::Refactor,
        edits,
    })
}

fn inline_cte(a: &Analysis, sql: &str, at: usize) -> Option<Fix> {
    let r = a.ref_at(at)?;
    let Resolution::Symbol(sym_id) = r.resolution else { return None };
    let sym = &a.symbols[sym_id];
    if sym.kind != crate::analysis::SymbolKind::Cte {
        return None;
    }
    let uses: Vec<_> =
        a.refs_to_symbol(sym_id).filter(|x| x.role != crate::analysis::RefRole::Declaration).collect();
    if uses.len() != 1 || uses[0].role != crate::analysis::RefRole::Table {
        return None;
    }
    let root = &a.parse.root;
    // locate the Cte node, its WithClause and the single TableName that uses it
    let mut with: Option<&Node> = None;
    let mut cte: Option<&Node> = None;
    root.walk(&mut |n| {
        if n.kind == Kind::WithClause && n.span.contains(&sym.span.start) && with.is_none() {
            with = Some(n);
            cte = n.children_of(Kind::Cte).find(|c| c.span.contains(&sym.span.start));
        }
    });
    let (with, cte) = (with?, cte?);
    if with.has_kw(sql, "RECURSIVE") || cte.child(Kind::ColumnList).is_some() {
        return None;
    }
    let body = cte.child(Kind::Query)?;
    let mut table_name: Option<&Node> = None;
    root.walk(&mut |n| {
        if n.kind == Kind::TableName && n.span.contains(&uses[0].span.start) && table_name.is_none() {
            table_name = Some(n);
        }
    });
    let tn = table_name?;
    let name_node = tn.child(Kind::Name)?;
    if tn.span.start >= with.span.start && tn.span.end <= cte.span.end {
        return None;
    }
    let cte_name = sym.name.clone();
    let replacement = if tn.child(Kind::Alias).is_some() {
        format!("({})", body.text(sql))
    } else {
        format!("({}) {}", body.text(sql), quote_if_needed(&cte_name, DatabaseKind::Postgres))
    };
    let mut edits = vec![Edit::replace(name_node.span.clone(), replacement)];
    let ctes: Vec<&Node> = with.children_of(Kind::Cte).collect();
    if ctes.len() == 1 {
        // drop the whole WITH clause and the whitespace after it
        let end = with.span.end + sql[with.span.end..].len() - sql[with.span.end..].trim_start().len();
        edits.push(Edit::delete(with.span.start..end));
    } else {
        let i = ctes.iter().position(|c| c.span == cte.span)?;
        if i + 1 < ctes.len() {
            // up to the start of the next CTE
            edits.push(Edit::delete(cte.span.start..ctes[i + 1].span.start));
        } else {
            // from the end of the previous CTE (taking its comma)
            edits.push(Edit::delete(ctes[i - 1].span.end..cte.span.end));
        }
    }
    Some(Fix { title: format!("Inline the CTE '{cte_name}'"), kind: FixKind::Refactor, edits })
}

// ---- keyword case

fn keyword_case(a: &Analysis, sql: &str, path: &[&Node]) -> Vec<Fix> {
    let Some(stmt) = path.iter().find(|n| n.kind == Kind::Statement) else { return Vec::new() };
    let mut upper: Vec<Edit> = Vec::new();
    let mut lower: Vec<Edit> = Vec::new();
    fn walk(n: &Node, sql: &str, upper: &mut Vec<Edit>, lower: &mut Vec<Edit>) {
        let identifier_parent = matches!(
            n.kind,
            Kind::Name
                | Kind::ColumnRef
                | Kind::Star
                | Kind::ColumnList
                | Kind::ColumnDef
                | Kind::ColumnDefList
                | Kind::TypeName
                | Kind::Literal
                | Kind::Param
                | Kind::Unknown
        );
        for c in &n.children {
            match c.kind {
                Kind::Token(TokenKind::Word) if !identifier_parent => {
                    let text = c.text(sql);
                    let in_alias = n.kind == Kind::Alias && !text.eq_ignore_ascii_case("AS");
                    if !in_alias && is_format_keyword(text) {
                        let u = text.to_ascii_uppercase();
                        let l = text.to_ascii_lowercase();
                        if u != text {
                            upper.push(Edit::replace(c.span.clone(), u));
                        }
                        if l != text {
                            lower.push(Edit::replace(c.span.clone(), l));
                        }
                    }
                }
                Kind::Token(_) => {}
                _ => walk(c, sql, upper, lower),
            }
        }
    }
    walk(stmt, sql, &mut upper, &mut lower);
    let _ = a;
    let mut out = Vec::new();
    if !upper.is_empty() {
        out.push(Fix {
            title: "Uppercase the keywords of this statement".to_string(),
            kind: FixKind::Source,
            edits: upper,
        });
    }
    if !lower.is_empty() {
        out.push(Fix {
            title: "Lowercase the keywords of this statement".to_string(),
            kind: FixKind::Source,
            edits: lower,
        });
    }
    out
}
