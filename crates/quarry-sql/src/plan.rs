//! Plan analysis: turns an EXPLAIN tree into specific, numbered findings.
//!
//! Every rule works from the database-neutral [`PlanNode`] fields plus, where the
//! driver could only supply free-form properties, the `extra` map. Rules are
//! tolerant of missing numbers (an un-analysed plan has estimates only) and use
//! the catalog, when available, to avoid suggesting indexes that already exist.

use crate::keywords::quote_if_needed;
use crate::lexer::{TokenKind, tokenize};
use crate::lint::Severity;
use quarry_core::{Catalog, DatabaseKind, Plan, PlanNode, Table};

#[derive(Debug, Clone, PartialEq)]
pub struct Insight {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Child indexes from the root to the node this is about.
    pub node_path: Vec<usize>,
    /// For example a `CREATE INDEX` statement.
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanSummary {
    pub total_ms: Option<f64>,
    pub total_cost: Option<f64>,
    pub slowest_node_path: Option<Vec<usize>>,
    pub node_count: usize,
}

/// Tables smaller than this are not worth an index however selective the filter.
const LARGE_TABLE_ROWS: f64 = 10_000.0;

fn fmt_int(v: f64) -> String {
    let n = v.round().max(0.0) as u64;
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn fmt_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

fn fmt_pct(f: f64) -> String {
    let p = f * 100.0;
    if p >= 99.95 {
        "100%".into()
    } else if p > 99.0 {
        format!("{p:.2}%")
    } else {
        format!("{p:.0}%")
    }
}

fn describe(n: &PlanNode) -> String {
    match (&n.relation, &n.alias) {
        (Some(r), Some(a)) if a != r => format!("{} on {r} {a}", n.node_type),
        (Some(r), _) => format!("{} on {r}", n.node_type),
        _ => n.node_type.clone(),
    }
}

fn extra_get<'a>(n: &'a PlanNode, key: &str) -> Option<&'a str> {
    n.extra.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

/// Any text the node carries (type, detail, relation-independent extras) that contains `needle`.
fn mentions(n: &PlanNode, needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase();
    n.node_type.to_ascii_lowercase().contains(&needle)
        || n.detail.as_deref().is_some_and(|d| d.to_ascii_lowercase().contains(&needle))
        || n.extra.values().any(|v| v.to_ascii_lowercase().contains(&needle))
}

fn parse_leading_number(s: &str) -> Option<f64> {
    let t: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    t.parse().ok()
}

/// `10240kB` / `Disk: 10240kB` -> 10240.0 (kB)
fn parse_kb_after(s: &str, marker: &str) -> Option<f64> {
    let pos = s.to_ascii_lowercase().find(&marker.to_ascii_lowercase())?;
    let rest = &s[pos + marker.len()..];
    let v = parse_leading_number(rest)?;
    let unit_part = rest
        .trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .trim_start()
        .to_ascii_lowercase();
    let mult = if unit_part.starts_with("gb") {
        1024.0 * 1024.0
    } else if unit_part.starts_with("mb") {
        1024.0
    } else if unit_part.starts_with('b') {
        1.0 / 1024.0
    } else {
        1.0
    };
    Some(v * mult)
}

fn lookup_table<'a>(catalog: Option<&'a Catalog>, relation: &str) -> Option<&'a Table> {
    let c = catalog?;
    let rel = relation.trim_matches('"');
    match rel.split_once('.') {
        Some((s, t)) => c.find_table(Some(s.trim_matches('"')), t.trim_matches('"')).map(|(_, t)| t),
        None => c.find_table(None, rel).map(|(_, t)| t),
    }
}

/// Columns a filter compares against constants: equality columns first, then range
/// columns, de-duplicated. Qualified references to *other* tables (join conditions)
/// are ignored.
pub(crate) fn filter_columns(filter: &str, node: &PlanNode, kind: DatabaseKind) -> Vec<String> {
    let toks: Vec<_> = tokenize(filter, kind).into_iter().filter(|t| !t.is_trivia()).collect();
    let text = |i: usize| toks[i].text(filter);
    let is_kw = |i: usize, w: &str| toks.get(i).is_some_and(|t| t.is_kw(filter, w));
    let own = |q: &str| {
        node.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(q))
            || node
                .relation
                .as_deref()
                .is_some_and(|r| r.rsplit('.').next().unwrap_or(r).eq_ignore_ascii_case(q))
    };
    let mut eq: Vec<String> = Vec::new();
    let mut range: Vec<String> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let is_name = matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent)
            && !(t.kind == TokenKind::Word
                && matches!(
                    text(i).to_ascii_uppercase().as_str(),
                    "AND"
                        | "OR"
                        | "NOT"
                        | "NULL"
                        | "TRUE"
                        | "FALSE"
                        | "IS"
                        | "IN"
                        | "ANY"
                        | "ALL"
                        | "LIKE"
                        | "ILIKE"
                        | "BETWEEN"
                        | "ARRAY"
                        | "CASE"
                        | "WHEN"
                        | "THEN"
                        | "ELSE"
                        | "END"
                        | "DISTINCT"
                        | "FROM"
                        | "SIMILAR"
                        | "TO"
                ));
        // Skip type names after `::`, function names and qualifiers.
        let after_cast = i > 0 && toks[i - 1].kind == TokenKind::Operator && text(i - 1) == "::";
        let is_call = toks.get(i + 1).is_some_and(|n| n.is_punct(filter, "("));
        let after_dot = i > 0 && toks[i - 1].is_punct(filter, ".");
        if is_name && is_call && !after_cast && !after_dot {
            // `lower(name) = 'x'` cannot use a plain index on `name`: skip the whole call.
            let mut depth = 0i32;
            let mut k = i + 1;
            while k < toks.len() {
                if toks[k].is_punct(filter, "(") {
                    depth += 1;
                } else if toks[k].is_punct(filter, ")") {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                k += 1;
            }
            i = k + 1;
            continue;
        }
        if !is_name || after_cast || after_dot {
            i += 1;
            continue;
        }
        // Resolve `qualifier.column`.
        let (col_idx, qualifier) = if toks.get(i + 1).is_some_and(|n| n.is_punct(filter, "."))
            && toks.get(i + 2).is_some_and(|n| matches!(n.kind, TokenKind::Word | TokenKind::QuotedIdent))
        {
            (i + 2, Some(crate::lexer::unquote_ident(text(i))))
        } else {
            (i, None)
        };
        if let Some(q) = &qualifier
            && !own(q)
        {
            i = col_idx + 1;
            continue;
        }
        let col = crate::lexer::unquote_ident(text(col_idx));
        // Look right past `)` and `::type` for the operator.
        let mut j = col_idx + 1;
        while j < toks.len()
            && (toks[j].is_punct(filter, ")") || (toks[j].kind == TokenKind::Operator && text(j) == "::"))
        {
            if toks[j].kind == TokenKind::Operator {
                j += 2; // `::` and the type name
                while toks.get(j).is_some_and(|t| {
                    t.is_punct(filter, "(")
                        || t.is_punct(filter, ")")
                        || t.kind == TokenKind::Number
                        || t.is_punct(filter, ",")
                }) {
                    j += 1;
                }
            } else {
                j += 1;
            }
        }
        let op = toks.get(j).map(|t| (t.kind, text(j).to_ascii_uppercase()));
        let mut class = match op {
            Some((TokenKind::Operator, o)) if o == "=" => Some(true),
            Some((TokenKind::Operator, o)) if matches!(o.as_str(), "<" | ">" | "<=" | ">=" | "~~" | "^@") => {
                Some(false)
            }
            Some((TokenKind::Word, w)) if w == "IS" && !is_kw(j + 1, "DISTINCT") => Some(true),
            Some((TokenKind::Word, w)) if w == "IN" => Some(true),
            Some((TokenKind::Word, w)) if matches!(w.as_str(), "BETWEEN" | "LIKE") => Some(false),
            _ => None,
        };
        if class.is_none() && i > 0 {
            // `42 = col`
            let mut k = i;
            while k > 0 && toks[k - 1].is_punct(filter, "(") {
                k -= 1;
            }
            if k >= 2
                && toks[k - 1].kind == TokenKind::Operator
                && text(k - 1) == "="
                && matches!(toks[k - 2].kind, TokenKind::Number | TokenKind::String)
            {
                class = Some(true);
            }
        }
        if let Some(is_eq) = class {
            let list = if is_eq { &mut eq } else { &mut range };
            if !list.iter().any(|c| c.eq_ignore_ascii_case(&col)) {
                list.push(col);
            }
        }
        i = col_idx + 1;
    }
    let mut out = eq;
    for c in range {
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&c)) {
            out.push(c);
        }
    }
    out.truncate(3);
    out
}

fn index_covers(table: &Table, cols: &[String]) -> Option<String> {
    table
        .indexes
        .iter()
        .find(|ix| {
            ix.columns.len() >= cols.len()
                && ix.columns[..cols.len()].iter().all(|c| cols.iter().any(|w| w.eq_ignore_ascii_case(c)))
        })
        .map(|ix| ix.name.clone())
}

fn create_index(kind: DatabaseKind, relation: &str, cols: &[String]) -> String {
    let rel_name = relation.rsplit('.').next().unwrap_or(relation).trim_matches('"');
    let mut name = format!("idx_{}_{}", rel_name, cols.join("_")).to_lowercase();
    name.retain(|c| c.is_ascii_alphanumeric() || c == '_');
    name.truncate(63);
    let target =
        relation.split('.').map(|p| quote_if_needed(p.trim_matches('"'), kind)).collect::<Vec<_>>().join(".");
    let cols: Vec<String> = cols.iter().map(|c| quote_if_needed(c, kind)).collect();
    format!("CREATE INDEX {name} ON {target} ({});", cols.join(", "))
}

fn analyze_stmt(kind: DatabaseKind, relation: &str) -> String {
    let target =
        relation.split('.').map(|p| quote_if_needed(p.trim_matches('"'), kind)).collect::<Vec<_>>().join(".");
    match kind {
        DatabaseKind::MySql => format!("ANALYZE TABLE {target};"),
        _ => format!("ANALYZE {target};"),
    }
}

fn relations_below(n: &PlanNode, out: &mut Vec<String>) {
    if let Some(r) = &n.relation
        && !out.contains(r)
    {
        out.push(r.clone());
    }
    for c in &n.children {
        relations_below(c, out);
    }
}

fn is_seq_scan(n: &PlanNode) -> bool {
    n.node_type.to_ascii_lowercase().contains("seq scan")
}

/// A scan that reads every row because it has no usable index (any dialect).
fn is_unindexed_scan(n: &PlanNode) -> bool {
    let t = n.node_type.to_ascii_lowercase();
    n.index.is_none()
        && (t.contains("seq scan")
            || t.contains("table scan")
            || (t.starts_with("scan") && !mentions(n, "index")))
}

struct Ctx<'a> {
    catalog: Option<&'a Catalog>,
    kind: DatabaseKind,
    analyzed: bool,
    total_ms: Option<f64>,
    out: Vec<Insight>,
    node_count: usize,
}

impl Ctx<'_> {
    fn push(
        &mut self,
        severity: Severity,
        title: String,
        detail: String,
        path: &[usize],
        suggestion: Option<String>,
    ) {
        self.out.push(Insight { severity, title, detail, node_path: path.to_vec(), suggestion });
    }
}

fn estimate_ratio(n: &PlanNode) -> Option<(f64, f64, f64)> {
    let (est, act) = (n.plan_rows?, n.actual_rows?);
    let ratio = est.max(1.0).max(act.max(1.0)) / est.max(1.0).min(act.max(1.0));
    (ratio >= 10.0 && est.max(act) >= 100.0).then_some((est, act, ratio))
}

fn visit(c: &mut Ctx, n: &PlanNode, path: &mut Vec<usize>) {
    c.node_count += 1;
    let mut seq_scan_flagged = false;

    // 1. Sequential scan on a large relation with a selective filter.
    if is_seq_scan(n) && n.filter.as_deref().is_some_and(|f| !f.trim().is_empty()) {
        let loops = n.loops.unwrap_or(1.0).max(1.0);
        let analyzed_read = match (n.actual_rows, n.rows_removed_by_filter) {
            (Some(a), r) if c.analyzed => Some((a + r.unwrap_or(0.0)) * loops),
            _ => None,
        };
        let table = n.relation.as_deref().and_then(|r| lookup_table(c.catalog, r));
        let table_rows = table.and_then(|t| t.row_estimate).map(|r| r as f64).or(analyzed_read);
        let large = table_rows.is_some_and(|r| r >= LARGE_TABLE_ROWS);
        let returned = n.actual_rows.map(|a| a * loops).or(n.plan_rows);
        let (selective, discarded_pct) = match (n.actual_rows, n.rows_removed_by_filter) {
            (Some(a), Some(r)) if c.analyzed && a + r > 0.0 => (r / (a + r) > 0.9, Some(r / (a + r))),
            _ => match (n.plan_rows, table_rows) {
                (Some(p), Some(t)) if t > 0.0 => (p / t < 0.1, Some(1.0 - (p / t).min(1.0))),
                _ => (false, None),
            },
        };
        if large && selective {
            seq_scan_flagged = true;
            let rel = n.relation.clone().unwrap_or_else(|| "table".into());
            let read = table_rows.unwrap_or(0.0);
            let filter = n.filter.clone().unwrap_or_default();
            let cols = filter_columns(&filter, n, c.kind);
            let covered = table.filter(|_| !cols.is_empty()).and_then(|t| index_covers(t, &cols));
            let severity = if read >= 1_000_000.0 || n.exclusive_ms().is_some_and(|m| m >= 1000.0) {
                Severity::Critical
            } else {
                Severity::Warning
            };
            let mut title = format!("Sequential scan reads {} rows of {rel}", fmt_int(read));
            if let Some(r) = returned {
                title.push_str(&format!(" to return {}", fmt_int(r)));
            }
            let mut detail = format!("The filter `{}` is applied to every row", filter.trim());
            if let Some(p) = discarded_pct {
                detail.push_str(&format!(" and discards {} of them", fmt_pct(p)));
            }
            detail.push('.');
            let suggestion = if cols.is_empty() {
                detail.push_str(" Add an index on the columns used in this filter.");
                None
            } else if let Some(ix) = covered {
                detail.push_str(&format!(
                    " An index on ({}) already exists ({ix}), so the planner is choosing not to use it: refresh statistics with ANALYZE, or check that the filter does not wrap the column in a function or cast.",
                    cols.join(", ")
                ));
                Some(analyze_stmt(c.kind, &rel))
            } else {
                detail.push_str(&format!(
                    " An index on ({}) lets the database jump straight to the matching rows.",
                    cols.join(", ")
                ));
                Some(create_index(c.kind, &rel, &cols))
            };
            c.push(severity, title, detail, path, suggestion);
        }
    }

    // 2. Estimate off by >= 10x, reported at the deepest affected node only (a bad
    // leaf estimate makes every ancestor wrong too).
    if c.analyzed
        && let Some((est, act, ratio)) = estimate_ratio(n)
        && !n.children.iter().any(|ch| estimate_ratio(ch).is_some())
    {
        let severity = if ratio >= 1000.0 { Severity::Critical } else { Severity::Warning };
        let dir = if act > est { "under-estimated" } else { "over-estimated" };
        let mut rels = Vec::new();
        relations_below(n, &mut rels);
        rels.truncate(2);
        let suggestion = (!rels.is_empty())
            .then(|| rels.iter().map(|r| analyze_stmt(c.kind, r)).collect::<Vec<_>>().join(" "));
        c.push(
            severity,
            format!("Row estimate is off by {:.0}× at {}", ratio, describe(n)),
            format!(
                "The planner {dir} this step: it expected {} rows per loop but {} came back. Stale or missing statistics are the usual cause, and a wrong estimate here can lead to a poor join order or join method.{}",
                fmt_int(est),
                fmt_int(act),
                if rels.is_empty() { String::new() } else { format!(" Refreshing statistics on {} usually fixes it.", rels.join(", ")) }
            ),
            path,
            suggestion,
        );
    }

    // 3. Unindexed scan executed many times (nested loop inner side).
    if let Some(loops) = n.loops
        && loops >= 100.0
        && is_unindexed_scan(n)
    {
        let rel = n.relation.clone().unwrap_or_else(|| "a table".into());
        let cols = n.filter.as_deref().map(|f| filter_columns(f, n, c.kind)).unwrap_or_default();
        let table = n.relation.as_deref().and_then(|r| lookup_table(c.catalog, r));
        let covered = table.filter(|_| !cols.is_empty()).and_then(|t| index_covers(t, &cols));
        let per_loop = n.actual_rows.map(|a| a + n.rows_removed_by_filter.unwrap_or(0.0));
        let mut detail = format!("{} runs {} times", describe(n), fmt_int(loops));
        if let Some(p) = per_loop {
            detail.push_str(&format!(
                ", reading about {} rows each time ({} in total)",
                fmt_int(p),
                fmt_int(p * loops)
            ));
        }
        detail.push_str(". Without an index each repetition scans the whole relation.");
        let suggestion =
            if cols.is_empty() || covered.is_some() { None } else { Some(create_index(c.kind, &rel, &cols)) };
        if suggestion.is_some() {
            detail.push_str(&format!(
                " An index on ({}) turns each repetition into a lookup.",
                cols.join(", ")
            ));
        }
        let severity = if loops >= 10_000.0 { Severity::Critical } else { Severity::Warning };
        c.push(
            severity,
            format!("Scan of {rel} repeated {} times in a nested loop", fmt_int(loops)),
            detail,
            path,
            suggestion,
        );
    }

    // 4. Sorts and hashes that spill to disk.
    let sort_method = extra_get(n, "Sort Method");
    if let Some(m) = sort_method
        && m.to_ascii_lowercase().contains("external")
    {
        let disk_kb = parse_kb_after(m, "Disk:")
            .or_else(|| extra_get(n, "Sort Space Used").and_then(|v| parse_kb_after(v, "")));
        let size = disk_kb.map(|k| format!(" ({} kB)", fmt_int(k))).unwrap_or_default();
        let suggestion = disk_kb.map(|k| {
            format!(
                "SET work_mem = '{}MB';",
                (((k / 1024.0) * 1.5).ceil().max(4.0) as u64).next_power_of_two()
            )
        });
        c.push(
            if disk_kb.is_some_and(|k| k >= 512.0 * 1024.0) { Severity::Critical } else { Severity::Warning },
            format!("Sort spilled to disk{size}"),
            format!("{} did not fit in work_mem and used an external merge sort on disk, which is much slower than sorting in memory. Raising work_mem for this session (or adding an index that returns rows already ordered) avoids the spill.", describe(n)),
            path,
            suggestion,
        );
    }
    let batches =
        extra_get(n, "Hash Batches").or_else(|| extra_get(n, "Batches")).and_then(parse_leading_number);
    if let Some(b) = batches
        && b > 1.0
    {
        let mem_kb = extra_get(n, "Peak Memory Usage")
            .or_else(|| extra_get(n, "Memory Usage"))
            .and_then(|v| parse_kb_after(v, ""));
        let suggestion = mem_kb.map(|k| {
            format!("SET work_mem = '{}MB';", (((k * b) / 1024.0).ceil().max(4.0) as u64).next_power_of_two())
        });
        c.push(
            Severity::Warning,
            format!("Hash split into {} batches (spilled to disk)", fmt_int(b)),
            format!("{} needed more memory than work_mem allows, so it processed the data in {} passes through temporary files. More work_mem lets it finish in one in-memory pass.", describe(n), fmt_int(b)),
            path,
            suggestion,
        );
    } else if let Some(disk) = extra_get(n, "Disk Usage").and_then(|v| parse_kb_after(v, ""))
        && disk > 0.0
    {
        c.push(
            Severity::Warning,
            format!("{} spilled {} kB to disk", n.node_type, fmt_int(disk)),
            "The aggregate exceeded work_mem and wrote temporary data to disk. Increasing work_mem avoids this.".into(),
            path,
            None,
        );
    }

    // 5. Node dominating runtime.
    if c.analyzed
        && c.node_count > 1
        && let (Some(own), Some(total)) = (n.exclusive_ms(), c.total_ms)
        && total > 0.0
        && own >= 1.0
        && own / total >= 0.5
    {
        let share = own / total;
        c.push(
            if share >= 0.8 && own >= 100.0 { Severity::Critical } else { Severity::Warning },
            format!("{} takes {} of the runtime", describe(n), fmt_pct(share)),
            format!("This step alone spent {} out of {} in total, excluding its children. It is the best place to look first when making the query faster.", fmt_ms(own), fmt_ms(total)),
            path,
            None,
        );
    }

    // 6. Filter discarding most rows (steps rule 1 did not already explain).
    if !seq_scan_flagged && let (Some(removed), Some(actual)) = (n.rows_removed_by_filter, n.actual_rows) {
        let loops = n.loops.unwrap_or(1.0).max(1.0);
        let (removed_total, kept_total) = (removed * loops, actual * loops);
        let read = removed_total + kept_total;
        if removed_total >= 1000.0 && read > 0.0 && removed_total / read > 0.9 {
            let cols = n.filter.as_deref().map(|f| filter_columns(f, n, c.kind)).unwrap_or_default();
            let rel = n.relation.clone();
            let table = rel.as_deref().and_then(|r| lookup_table(c.catalog, r));
            let covered = table.filter(|_| !cols.is_empty()).and_then(|t| index_covers(t, &cols));
            let suggestion = match (&rel, cols.is_empty(), covered) {
                (Some(r), false, None) => Some(create_index(c.kind, r, &cols)),
                _ => None,
            };
            c.push(
                if removed_total >= 100_000.0 { Severity::Warning } else { Severity::Info },
                format!("Filter removes {} of the rows read by {}", fmt_pct(removed_total / read), describe(n)),
                format!(
                    "{} rows were read and only {} kept{}. Pushing the condition into an index lookup would avoid reading the discarded rows.",
                    fmt_int(read),
                    fmt_int(kept_total),
                    n.filter.as_deref().map(|f| format!(" (filter `{}`)", f.trim())).unwrap_or_default()
                ),
                path,
                suggestion,
            );
        }
    }

    // 7. Joins without a join condition.
    cartesian_check(c, n, path);

    // 8. MySQL / MariaDB.
    if c.kind == DatabaseKind::MySql {
        mysql_checks(c, n, path);
    }
    // 9. SQLite.
    if c.kind == DatabaseKind::Sqlite {
        sqlite_checks(c, n, path);
    }

    for (i, ch) in n.children.iter().enumerate() {
        path.push(i);
        visit(c, ch, path);
        path.pop();
    }
}

fn has_join_condition(n: &PlanNode) -> bool {
    n.filter.is_some()
        || ["Join Filter", "Hash Cond", "Merge Cond", "Index Cond", "Join Condition", "Recheck Cond"]
            .iter()
            .any(|k| extra_get(n, k).is_some())
}

fn cartesian_check(c: &mut Ctx, n: &PlanNode, path: &[usize]) {
    let t = n.node_type.to_ascii_lowercase();
    let is_join =
        t.contains("nested loop") || t.contains("join") || t.contains("cartesian") || t.contains("cross");
    if !is_join || n.children.len() < 2 {
        return;
    }
    let explicit = t.contains("cartesian") || t.contains("cross join") || mentions(n, "cartesian");
    let mut conds = has_join_condition(n);
    // A parameterised inner scan (`Index Cond: (o.customer_id = c.id)`) is a join condition.
    for ch in &n.children {
        let mut found = false;
        ch.walk(&mut |d, _| {
            found |=
                extra_get(d, "Index Cond").is_some() || d.filter.is_some() && d.loops.unwrap_or(1.0) > 1.0
        });
        conds |= found;
    }
    if t.contains("hash join") || t.contains("merge join") {
        // These cannot execute without a condition in the first place.
        if !explicit {
            return;
        }
    }
    let (a, b) = (&n.children[0], &n.children[1]);
    let outer = if c.analyzed { a.actual_rows.map(|r| r * a.loops.unwrap_or(1.0)) } else { a.plan_rows };
    let inner = if c.analyzed { b.actual_rows } else { b.plan_rows };
    let produced = if c.analyzed { n.actual_rows.map(|r| r * n.loops.unwrap_or(1.0)) } else { n.plan_rows };
    let (Some(outer), Some(inner), Some(produced)) = (outer, inner, produced) else {
        if explicit {
            c.push(
                Severity::Critical,
                format!("{} has no join condition", n.node_type),
                "Every row of one input is paired with every row of the other. Check that each JOIN has an ON clause linking the tables.".into(),
                path,
                None,
            );
        }
        return;
    };
    let product = outer * inner;
    if (explicit || !conds) && product >= 1000.0 && produced >= 0.9 * product {
        c.push(
            Severity::Critical,
            format!("Possible cartesian product: {} × {} = {} rows", fmt_int(outer), fmt_int(inner), fmt_int(product)),
            format!(
                "{} produces {} rows, which is every combination of its inputs, and no join condition was found. A missing or mistyped ON clause (or a comma join without a WHERE predicate) is the usual cause.",
                describe(n),
                fmt_int(produced)
            ),
            path,
            Some("Add the join predicate, e.g. JOIN … ON a.key = b.key".into()),
        );
    }
}

fn mysql_checks(c: &mut Ctx, n: &PlanNode, path: &[usize]) {
    let rel = n.relation.clone().unwrap_or_else(|| "the table".into());
    let table = n.relation.as_deref().and_then(|r| lookup_table(c.catalog, r));
    let rows = n.plan_rows.or(table.and_then(|t| t.row_estimate).map(|r| r as f64));
    if mentions(n, "Using filesort") {
        c.push(
            Severity::Warning,
            format!("Using filesort on {rel}"),
            format!(
                "MySQL cannot return rows in ORDER BY order from an index, so it sorts them separately{}. An index whose columns match the ORDER BY (after any equality filters) removes the sort.",
                rows.map(|r| format!(" ({} rows estimated)", fmt_int(r))).unwrap_or_default()
            ),
            path,
            None,
        );
    }
    if mentions(n, "Using temporary") {
        c.push(
            Severity::Warning,
            format!("Using temporary table on {rel}"),
            "MySQL materialises an intermediate temporary table for this GROUP BY / DISTINCT / UNION. An index covering the grouped columns lets it aggregate in index order instead.".into(),
            path,
            None,
        );
    }
    let access = extra_get(n, "type")
        .or_else(|| extra_get(n, "access_type"))
        .map(|s| s.to_string())
        .or_else(|| Some(n.node_type.clone()))
        .or_else(|| n.detail.clone());
    let is_all = access.as_deref().is_some_and(|a| a.trim().eq_ignore_ascii_case("ALL"))
        || n.detail.as_deref().is_some_and(|d| d.trim().eq_ignore_ascii_case("ALL"));
    if is_all && rows.is_none_or(|r| r >= 1000.0) {
        let cols = n.filter.as_deref().map(|f| filter_columns(f, n, c.kind)).unwrap_or_default();
        let covered = table.filter(|_| !cols.is_empty()).and_then(|t| index_covers(t, &cols));
        let suggestion = match (&n.relation, cols.is_empty(), covered) {
            (Some(r), false, None) => Some(create_index(c.kind, r, &cols)),
            _ => None,
        };
        c.push(
            if rows.is_some_and(|r| r >= 100_000.0) { Severity::Critical } else { Severity::Warning },
            format!("Full table scan (type ALL) on {rel}"),
            format!(
                "MySQL reads every row of {rel}{}. If the query filters or joins on indexed columns this usually means no usable index exists for the condition.",
                rows.map(|r| format!(" (about {} rows)", fmt_int(r))).unwrap_or_default()
            ),
            path,
            suggestion,
        );
    }
}

fn sqlite_checks(c: &mut Ctx, n: &PlanNode, path: &[usize]) {
    let up = n.node_type.to_ascii_uppercase();
    let scan_like = up.starts_with("SCAN") || up == "TABLE SCAN";
    let rel = n.relation.clone();
    if scan_like
        && n.index.is_none()
        && !mentions(n, "index")
        && !mentions(n, "constant row")
        && !mentions(n, "subquery")
        && let Some(rel) = rel
            .clone()
            .or_else(|| n.detail.as_deref().and_then(|d| d.split_whitespace().nth(1)).map(|s| s.to_string()))
    {
        let table = lookup_table(c.catalog, &rel);
        let small = table.and_then(|t| t.row_estimate).is_some_and(|r| r < 1000);
        if !small {
            let cols = n.filter.as_deref().map(|f| filter_columns(f, n, c.kind)).unwrap_or_default();
            let covered = table.filter(|_| !cols.is_empty()).and_then(|t| index_covers(t, &cols));
            let suggestion = if cols.is_empty() || covered.is_some() {
                None
            } else {
                Some(create_index(c.kind, &rel, &cols))
            };
            c.push(
                Severity::Warning,
                format!("SCAN {rel} reads the whole table"),
                format!(
                    "SQLite visits every row of {rel}{}. A SEARCH using an index would only touch the matching rows.",
                    table.and_then(|t| t.row_estimate).map(|r| format!(" (about {} rows)", fmt_int(r as f64))).unwrap_or_default()
                ),
                path,
                suggestion,
            );
        }
    }
    for (needle, what) in [
        ("USE TEMP B-TREE FOR ORDER BY", "ORDER BY"),
        ("USE TEMP B-TREE FOR GROUP BY", "GROUP BY"),
        ("USE TEMP B-TREE FOR DISTINCT", "DISTINCT"),
    ] {
        if mentions(n, needle) {
            c.push(
                Severity::Info,
                format!("Temporary B-tree built for {what}"),
                format!("SQLite has to build and sort a temporary index to satisfy {what}. An index on the {what} columns lets it read rows in the right order directly."),
                path,
                None,
            );
        }
    }
    // Generic "USE TEMP B-TREE" with no recognised purpose.
    if mentions(n, "USE TEMP B-TREE")
        && !["ORDER BY", "GROUP BY", "DISTINCT"]
            .iter()
            .any(|w| mentions(n, &format!("USE TEMP B-TREE FOR {w}")))
    {
        c.push(
            Severity::Info,
            "Temporary B-tree built".into(),
            "SQLite builds a temporary index at run time; a permanent index on the same columns would avoid doing that on every execution.".into(),
            path,
            None,
        );
    }
}

fn slowest(plan: &Plan) -> Option<Vec<usize>> {
    fn go(n: &PlanNode, path: &mut Vec<usize>, best: &mut Option<(f64, Vec<usize>)>, by_time: bool) {
        let own = if by_time {
            n.exclusive_ms()
        } else {
            n.total_cost.map(|t| (t - n.children.iter().filter_map(|c| c.total_cost).sum::<f64>()).max(0.0))
        };
        if let Some(v) = own
            && best.as_ref().is_none_or(|(b, _)| v > *b)
        {
            *best = Some((v, path.clone()));
        }
        for (i, ch) in n.children.iter().enumerate() {
            path.push(i);
            go(ch, path, best, by_time);
            path.pop();
        }
    }
    let mut any_time = false;
    plan.root.walk(&mut |n, _| any_time |= n.actual_total_ms.is_some());
    let mut best = None;
    go(&plan.root, &mut Vec::new(), &mut best, any_time);
    best.map(|(_, p)| p)
}

/// Summarise a plan and list what is worth fixing, most severe first.
pub fn analyze_plan(
    plan: &Plan,
    catalog: Option<&Catalog>,
    kind: DatabaseKind,
) -> (PlanSummary, Vec<Insight>) {
    let total_ms = plan.execution_time_ms.or(plan.root.actual_total_ms);
    let mut ctx = Ctx {
        catalog,
        kind,
        analyzed: plan.analyzed,
        total_ms: plan.root.actual_total_ms.or(plan.execution_time_ms),
        out: Vec::new(),
        node_count: 0,
    };
    visit(&mut ctx, &plan.root, &mut Vec::new());
    let mut insights = std::mem::take(&mut ctx.out);
    let sev = |s: Severity| match s {
        Severity::Critical => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    };
    insights.sort_by(|a, b| {
        sev(a.severity)
            .cmp(&sev(b.severity))
            .then_with(|| a.node_path.cmp(&b.node_path))
            .then_with(|| a.title.cmp(&b.title))
    });
    let summary = PlanSummary {
        total_ms,
        total_cost: plan.root.total_cost,
        slowest_node_path: slowest(plan),
        node_count: ctx.node_count,
    };
    (summary, insights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use DatabaseKind::*;

    fn node(t: &str) -> PlanNode {
        PlanNode { node_type: t.into(), ..Default::default() }
    }

    fn scan(rel: &str, filter: &str) -> PlanNode {
        PlanNode {
            node_type: "Seq Scan".into(),
            relation: Some(rel.into()),
            filter: Some(filter.into()),
            ..Default::default()
        }
    }

    fn plan(root: PlanNode, analyzed: bool) -> Plan {
        Plan { root, analyzed, planning_time_ms: None, execution_time_ms: None, raw: String::new() }
    }

    fn insights(root: PlanNode, analyzed: bool, kind: DatabaseKind) -> Vec<Insight> {
        let cat = fixtures::shop();
        analyze_plan(&plan(root, analyzed), Some(&cat), kind).1
    }

    #[test]
    fn seq_scan_with_selective_filter_suggests_index() {
        let mut n = scan("orders", "((status)::text = 'open'::text)");
        n.actual_rows = Some(300.0);
        n.rows_removed_by_filter = Some(1_249_700.0);
        n.plan_rows = Some(350.0);
        n.loops = Some(1.0);
        let got = insights(n, true, Postgres);
        let i = got.iter().find(|i| i.title.starts_with("Sequential scan")).expect("insight");
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.title.contains("1,250,000") && i.title.contains("to return 300"), "{}", i.title);
        assert!(i.detail.contains("100%") || i.detail.contains("99.9"), "{}", i.detail);
        assert_eq!(i.suggestion.as_deref(), Some("CREATE INDEX idx_orders_status ON orders (status);"));
        assert_eq!(i.node_path, Vec::<usize>::new());
    }

    #[test]
    fn existing_index_is_not_suggested_again() {
        let mut n = scan("orders", "(customer_id = 42)");
        n.actual_rows = Some(10.0);
        n.rows_removed_by_filter = Some(1_249_990.0);
        let got = insights(n, true, Postgres);
        let i = got.iter().find(|i| i.title.starts_with("Sequential scan")).unwrap();
        assert!(i.detail.contains("orders_customer_id_idx"), "{}", i.detail);
        assert!(i.suggestion.as_deref().is_some_and(|s| s.starts_with("ANALYZE")));
    }

    #[test]
    fn composite_index_prefers_equality_columns() {
        let mut n = scan(
            "orders",
            "((status = 'open'::text) AND (placed_at > '2024-01-01'::date) AND (store_id = 3))",
        );
        n.actual_rows = Some(10.0);
        n.rows_removed_by_filter = Some(1_249_990.0);
        let got = insights(n, true, Postgres);
        let s = got[0].suggestion.clone().unwrap();
        assert_eq!(
            s,
            "CREATE INDEX idx_orders_status_store_id_placed_at ON orders (status, store_id, placed_at);"
        );
    }

    #[test]
    fn small_tables_and_unselective_filters_are_quiet() {
        let mut n = scan("stores", "(name = 'x')");
        n.actual_rows = Some(1.0);
        n.rows_removed_by_filter = Some(41.0);
        assert!(insights(n, true, Postgres).is_empty());
        let mut n = scan("orders", "(total > 1)");
        n.actual_rows = Some(800_000.0);
        n.rows_removed_by_filter = Some(450_000.0);
        assert!(insights(n, true, Postgres).iter().all(|i| !i.title.starts_with("Sequential")));
    }

    #[test]
    fn unanalyzed_plan_uses_catalog_row_counts() {
        let mut n = scan("orders", "(status = 'open')");
        n.plan_rows = Some(500.0);
        let got = insights(n, false, Postgres);
        assert!(got.iter().any(|i| i.title.contains("1,250,000")));
    }

    #[test]
    fn filter_parser_ignores_other_tables_and_casts() {
        let mut n = node("Seq Scan");
        n.alias = Some("o".into());
        let cols = filter_columns(
            "((o.customer_id = c.id) AND ((o.status)::text = ANY ('{a,b}'::text[])) AND (lower(o.name) = 'x') AND (42 = o.store_id))",
            &n,
            Postgres,
        );
        assert_eq!(cols, vec!["customer_id", "status", "store_id"]);
        let cols = filter_columns(
            "(a > 1 OR b IS NULL) AND c LIKE 'x%' AND d BETWEEN 1 AND 2",
            &node("Seq Scan"),
            Postgres,
        );
        // Capped at three columns: more rarely helps and bloats the index.
        assert_eq!(cols, vec!["b", "a", "c"]);
    }

    #[test]
    fn estimate_off_by_10x_reports_deepest_node_and_analyze() {
        let mut leaf = PlanNode {
            node_type: "Index Scan".into(),
            relation: Some("orders".into()),
            ..Default::default()
        };
        leaf.plan_rows = Some(100.0);
        leaf.actual_rows = Some(5000.0);
        leaf.loops = Some(1.0);
        let mut root = node("Sort");
        root.plan_rows = Some(100.0);
        root.actual_rows = Some(5000.0);
        root.children.push(leaf);
        let got = insights(root, true, Postgres);
        let est: Vec<_> = got.iter().filter(|i| i.title.starts_with("Row estimate")).collect();
        assert_eq!(est.len(), 1, "{got:?}");
        assert_eq!(est[0].node_path, vec![0]);
        assert!(est[0].title.contains("50×"));
        assert!(est[0].detail.contains("under-estimated") && est[0].detail.contains("5,000"));
        assert_eq!(est[0].suggestion.as_deref(), Some("ANALYZE orders;"));
        assert_eq!(est[0].severity, Severity::Warning);
    }

    #[test]
    fn small_estimate_errors_are_ignored_and_mysql_uses_analyze_table() {
        let mut n = node("Scan");
        n.plan_rows = Some(1.0);
        n.actual_rows = Some(40.0);
        assert!(insights(n, true, Postgres).is_empty());
        let mut n =
            PlanNode { node_type: "Scan".into(), relation: Some("orders".into()), ..Default::default() };
        n.plan_rows = Some(5000.0);
        n.actual_rows = Some(10.0);
        let got = insights(n, true, MySql);
        assert_eq!(got[0].suggestion.as_deref(), Some("ANALYZE TABLE orders;"));
        assert!(got[0].detail.contains("over-estimated"));
    }

    #[test]
    fn nested_loop_with_many_inner_scans() {
        let mut inner = scan("reviews", "(product_id = p.id)");
        inner.loops = Some(25_000.0);
        inner.actual_rows = Some(3.0);
        inner.rows_removed_by_filter = Some(3_399_997.0);
        let mut outer = node("Index Scan");
        outer.relation = Some("orders".into());
        let mut nl = node("Nested Loop");
        nl.children = vec![outer, inner];
        nl.filter = None;
        let got = insights(nl, true, Postgres);
        let i = got.iter().find(|i| i.title.starts_with("Scan of reviews repeated")).expect("loop insight");
        assert_eq!(i.severity, Severity::Critical);
        assert_eq!(i.node_path, vec![1]);
        assert!(i.title.contains("25,000"));
        assert!(i.suggestion.as_deref().unwrap().contains("ON reviews (product_id)"));
    }

    #[test]
    fn sort_spill_to_disk() {
        let mut s = node("Sort");
        s.extra.insert("Sort Method".into(), "external merge  Disk: 40960kB".into());
        let got = insights(s, true, Postgres);
        assert_eq!(got[0].severity, Severity::Warning);
        assert!(got[0].title.contains("40,960 kB"));
        assert_eq!(got[0].suggestion.as_deref(), Some("SET work_mem = '64MB';"));
        let mut ok = node("Sort");
        ok.extra.insert("Sort Method".into(), "quicksort  Memory: 25kB".into());
        assert!(insights(ok, true, Postgres).is_empty());
    }

    #[test]
    fn hash_batches_spill() {
        let mut h = node("Hash");
        h.extra.insert("Hash Batches".into(), "8".into());
        h.extra.insert("Peak Memory Usage".into(), "4096kB".into());
        let got = insights(h, true, Postgres);
        assert!(got[0].title.contains("8 batches"));
        assert_eq!(got[0].suggestion.as_deref(), Some("SET work_mem = '32MB';"));
        let mut single = node("Hash");
        single.extra.insert("Batches".into(), "1".into());
        assert!(insights(single, true, Postgres).is_empty());
    }

    #[test]
    fn dominating_node() {
        let mut leaf = node("Seq Scan");
        leaf.relation = Some("stores".into());
        leaf.actual_total_ms = Some(900.0);
        let mut other = node("Index Scan");
        other.actual_total_ms = Some(20.0);
        let mut root = node("Hash Join");
        root.actual_total_ms = Some(1000.0);
        root.children = vec![leaf, other];
        let got = insights(root, true, Postgres);
        let d = got.iter().find(|i| i.title.contains("of the runtime")).expect("dominant");
        assert_eq!(d.node_path, vec![0]);
        assert!(d.title.starts_with("Seq Scan on stores takes 90%"), "{}", d.title);
        assert_eq!(d.severity, Severity::Critical);
        assert!(d.detail.contains("900 ms") && d.detail.contains("1.00 s"));
    }

    #[test]
    fn dominance_needs_multiple_nodes_and_real_time() {
        let mut only = node("Seq Scan");
        only.actual_total_ms = Some(500.0);
        assert!(insights(only, true, Postgres).iter().all(|i| !i.title.contains("runtime")));
        let mut leaf = node("Seq Scan");
        leaf.actual_total_ms = Some(0.4);
        let mut root = node("Limit");
        root.actual_total_ms = Some(0.5);
        root.children.push(leaf);
        assert!(insights(root, true, Postgres).iter().all(|i| !i.title.contains("runtime")));
    }

    #[test]
    fn filter_discarding_most_rows_on_index_scan() {
        let mut n = node("Index Scan");
        n.relation = Some("orders".into());
        n.index = Some("orders_customer_id_idx".into());
        n.filter = Some("(status = 'open'::text)".into());
        n.actual_rows = Some(200.0);
        n.rows_removed_by_filter = Some(199_800.0);
        n.loops = Some(1.0);
        let got = insights(n, true, Postgres);
        let i = got.iter().find(|i| i.title.starts_with("Filter removes")).unwrap();
        assert!(i.title.contains("100%") || i.title.contains("99.9"), "{}", i.title);
        assert_eq!(i.severity, Severity::Warning);
        assert!(i.detail.contains("200 kept") || i.detail.contains("only 200 kept"));
        assert_eq!(i.suggestion.as_deref(), Some("CREATE INDEX idx_orders_status ON orders (status);"));
        let mut mild = node("Index Scan");
        mild.actual_rows = Some(60_000.0);
        mild.rows_removed_by_filter = Some(40_000.0);
        assert!(insights(mild, true, Postgres).is_empty());
    }

    #[test]
    fn cartesian_product_detected() {
        let mut a = node("Seq Scan");
        a.actual_rows = Some(500.0);
        a.loops = Some(1.0);
        let mut b = node("Seq Scan");
        b.actual_rows = Some(400.0);
        b.loops = Some(500.0);
        let mut nl = node("Nested Loop");
        nl.actual_rows = Some(200_000.0);
        nl.loops = Some(1.0);
        nl.children = vec![a, b];
        let got = insights(nl, true, Postgres);
        let i = got.iter().find(|i| i.title.starts_with("Possible cartesian")).expect("cartesian");
        assert_eq!(i.severity, Severity::Critical);
        assert!(i.title.contains("500 × 400 = 200,000"), "{}", i.title);
    }

    #[test]
    fn join_with_condition_is_not_cartesian() {
        let mut a = node("Seq Scan");
        a.actual_rows = Some(500.0);
        a.loops = Some(1.0);
        let mut b = node("Index Scan");
        b.actual_rows = Some(400.0);
        b.loops = Some(500.0);
        b.extra.insert("Index Cond".into(), "(customer_id = c.id)".into());
        let mut nl = node("Nested Loop");
        nl.actual_rows = Some(200_000.0);
        nl.children = vec![a, b];
        assert!(insights(nl, true, Postgres).iter().all(|i| !i.title.contains("cartesian")));
        let mut hj = node("Hash Join");
        hj.extra.insert("Hash Cond".into(), "(a.id = b.id)".into());
        hj.children = vec![node("Seq Scan"), node("Hash")];
        assert!(insights(hj, true, Postgres).is_empty());
    }

    #[test]
    fn explicit_cross_join_node_without_numbers() {
        let mut n = node("Cartesian Product");
        n.children = vec![node("Seq Scan"), node("Seq Scan")];
        let got = insights(n, false, Postgres);
        assert_eq!(got[0].severity, Severity::Critical);
        assert!(got[0].title.contains("no join condition"));
    }

    #[test]
    fn mysql_filesort_temporary_and_all() {
        let mut n = node("SIMPLE");
        n.relation = Some("orders".into());
        n.extra.insert("type".into(), "ALL".into());
        n.extra.insert("Extra".into(), "Using where; Using temporary; Using filesort".into());
        n.plan_rows = Some(1_200_000.0);
        n.filter = Some("`orders`.`status` = 'open'".into());
        let got = insights(n, false, MySql);
        let titles: Vec<&str> = got.iter().map(|i| i.title.as_str()).collect();
        assert!(titles.iter().any(|t| t.starts_with("Using filesort")), "{titles:?}");
        assert!(titles.iter().any(|t| t.starts_with("Using temporary")));
        let all = got.iter().find(|i| i.title.starts_with("Full table scan")).unwrap();
        assert_eq!(all.severity, Severity::Critical);
        assert!(all.detail.contains("1,200,000"));
        assert_eq!(all.suggestion.as_deref(), Some("CREATE INDEX idx_orders_status ON orders (status);"));
    }

    #[test]
    fn mysql_small_full_scan_is_ignored() {
        let mut n = node("SIMPLE");
        n.relation = Some("stores".into());
        n.extra.insert("type".into(), "ALL".into());
        n.plan_rows = Some(40.0);
        assert!(insights(n, false, MySql).is_empty());
        let mut ok = node("SIMPLE");
        ok.extra.insert("type".into(), "ref".into());
        ok.plan_rows = Some(1_000_000.0);
        assert!(insights(ok, false, MySql).is_empty());
    }

    #[test]
    fn sqlite_scan_and_temp_btree() {
        let mut n = node("SCAN");
        n.relation = Some("orders".into());
        n.detail = Some("SCAN orders".into());
        let got = insights(n, false, Sqlite);
        assert_eq!(got[0].severity, Severity::Warning);
        assert!(got[0].title.contains("SCAN orders") && got[0].detail.contains("1,250,000"));
        let mut indexed = node("SEARCH");
        indexed.relation = Some("orders".into());
        indexed.detail = Some("SEARCH orders USING INDEX orders_customer_id_idx (customer_id=?)".into());
        assert!(insights(indexed, false, Sqlite).is_empty());
        let mut tmp = node("USE TEMP B-TREE FOR ORDER BY");
        tmp.detail = Some("USE TEMP B-TREE FOR ORDER BY".into());
        let got = insights(tmp, false, Sqlite);
        assert_eq!(got[0].severity, Severity::Info);
        assert!(got[0].title.contains("ORDER BY"));
    }

    #[test]
    fn sqlite_scan_of_small_table_is_ignored() {
        let mut n = node("SCAN");
        n.relation = Some("stores".into());
        assert!(insights(n, false, Sqlite).is_empty());
        let mut sub = node("SCAN SUBQUERY");
        sub.detail = Some("SCAN SUBQUERY 1".into());
        assert!(insights(sub, false, Sqlite).is_empty());
    }

    #[test]
    fn summary_numbers_and_slowest_node() {
        let mut a = node("Seq Scan");
        a.actual_total_ms = Some(5.0);
        a.total_cost = Some(10.0);
        let mut b = node("Index Scan");
        b.actual_total_ms = Some(80.0);
        b.total_cost = Some(20.0);
        let mut root = node("Hash Join");
        root.actual_total_ms = Some(100.0);
        root.total_cost = Some(50.0);
        root.children = vec![a, b];
        let mut p = plan(root, true);
        p.execution_time_ms = Some(101.5);
        let (s, _) = analyze_plan(&p, None, Postgres);
        assert_eq!(s.total_ms, Some(101.5));
        assert_eq!(s.total_cost, Some(50.0));
        assert_eq!(s.node_count, 3);
        assert_eq!(s.slowest_node_path, Some(vec![1]));
    }

    #[test]
    fn slowest_by_cost_when_not_analyzed() {
        let mut a = node("Seq Scan");
        a.total_cost = Some(90.0);
        let mut root = node("Limit");
        root.total_cost = Some(100.0);
        root.children = vec![a];
        let (s, _) = analyze_plan(&plan(root, false), None, Postgres);
        assert_eq!(s.slowest_node_path, Some(vec![0]));
        assert_eq!(s.total_ms, None);
        let (s, i) = analyze_plan(&plan(node("Result"), false), None, Postgres);
        assert_eq!((s.slowest_node_path, i.len(), s.node_count), (None, 0, 1));
    }

    #[test]
    fn insights_sorted_most_severe_first() {
        let mut n = scan("orders", "(status = 'open')");
        n.actual_rows = Some(5.0);
        n.rows_removed_by_filter = Some(1_249_995.0);
        n.plan_rows = Some(50_000.0);
        n.loops = Some(1.0);
        let got = insights(n, true, Postgres);
        assert!(got.len() >= 2);
        assert!(
            got.windows(2).all(
                |w| (w[0].severity as u8) >= (w[1].severity as u8) || w[0].severity == Severity::Critical
            )
        );
        assert_eq!(got[0].severity, Severity::Critical);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_int(1_250_000.0), "1,250,000");
        assert_eq!(fmt_int(999.4), "999");
        assert_eq!(fmt_pct(0.9997), "100%");
        assert_eq!(fmt_pct(0.995), "99.50%");
        assert_eq!(fmt_pct(0.93), "93%");
        assert_eq!(fmt_ms(1500.0), "1.50 s");
        assert_eq!(fmt_ms(12.0), "12 ms");
        assert_eq!(fmt_ms(1.25), "1.2 ms");
    }
}
