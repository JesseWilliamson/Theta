//! The migration script that makes the target look like the source.
//!
//! # Order
//!
//! Statements are emitted in phases, so that every statement finds what it needs and nothing is dropped while
//! something still points at it:
//!
//! 1. drop views that are gone or changed (a view may read any table below);
//! 2. drop foreign keys that are gone or changed, and every foreign key of a table about to be dropped;
//! 3. drop tables that are gone;
//! 4. create new tables (without their foreign keys on products that can add them later; referenced tables
//!    first, which is all SQLite needs because it checks references only when rows are written);
//! 5. alter changed tables (columns, primary and unique keys, checks, indexes: `ddl::diff`, and on SQLite its
//!    documented table rebuild);
//! 6. add foreign keys of new tables and the new or changed foreign keys of old ones;
//! 7. create views, a view that mentions another new view after it.
//!
//! # Destructive statements
//!
//! A statement that loses data or structure (drop table, drop column, a narrowing type change, a dropped or
//! changed constraint or index) is *held back* unless the caller asks for it: it is emitted as a comment
//! starting `-- destructive:`. Held back is not the same as filtered: later statements may depend on a dropped
//! thing, so each changed table is generated twice. First the **safe target**, the source with every
//! destructive change undone (removed columns kept, narrowing types kept, changed constraints kept as they
//! are), is diffed from the target and emitted live; then the step from the safe target to the source is
//! emitted as comments. Uncommenting the second part gives a script that is correct in order. With the toggle on,
//! the table is diffed straight to the source, which is shorter (one SQLite rebuild instead of two).

use super::{ObjectKind, SchemaDiff, Side, Status, column_attributes, norm_default, strip_schema};
use crate::ddl::{self, CheckDraft, Flag, ForeignKeyDraft, IndexDraft, KeyDraft, Statement, TableDraft};
use crate::keywords::qualified_name;
use std::collections::HashMap;
use theta_core::{DatabaseKind, Family, Table};

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptStatement {
    pub sql: String,
    pub explanation: String,
    /// Written as a comment: it is destructive and the caller did not ask for destructive statements.
    pub commented: bool,
    /// The object it is about, for the headings of the script text.
    pub object: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Script {
    pub statements: Vec<ScriptStatement>,
    /// Differences the script cannot express, in words; also printed as comments.
    pub manual: Vec<String>,
}

impl Script {
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    pub fn commented_count(&self) -> usize {
        self.statements.iter().filter(|s| s.commented).count()
    }

    pub fn live_count(&self) -> usize {
        self.statements.iter().filter(|s| !s.commented).count()
    }

    /// The text for a console. Nothing in it has been run.
    pub fn render(&self, kind: DatabaseKind, source: &str, target: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("-- Migration script: make {target} look like {source} ({}).\n", kind.label()));
        out.push_str(
            "-- Theta generated this text; nothing in it has been run. Read it before you run it.\n",
        );
        if kind.family() == Family::Sqlite {
            out.push_str("-- SQLite: run this in autocommit mode (a console's default). `PRAGMA foreign_keys` has no effect inside a transaction, and the table rebuilds below depend on it.\n");
        } else if ddl::transactional_ddl(kind) {
            out.push_str(
                "-- This product has transactional DDL: run the whole script inside one transaction.\n",
            );
        } else {
            out.push_str("-- This product commits after every DDL statement: a failure half-way leaves the changes before it in place.\n");
        }
        let held = self.commented_count();
        if held > 0 {
            out.push_str(&format!(
                "-- {} {} commented out and marked `destructive:`; uncomment them in order, or generate the script again with destructive statements included.\n",
                theta_core::text::plural_s(held as u64, "destructive statement"),
                if held == 1 { "is" } else { "are" }
            ));
        }
        for m in &self.manual {
            out.push_str(&format!("-- manual: {m}\n"));
        }
        out.push('\n');
        let mut object = String::new();
        for s in &self.statements {
            if s.object != object {
                object = s.object.clone();
                out.push_str(&format!("-- {object}\n"));
            }
            let sql = s.sql.trim_end().trim_end_matches(';');
            if s.commented {
                out.push_str(&format!("-- destructive: {}\n", s.explanation));
                // The terminating `;` stays on the last commented line so that removing the comment marks
                // gives a complete statement.
                let n = sql.lines().count();
                for (i, line) in sql.lines().enumerate() {
                    out.push_str(&format!("-- {line}{}\n", if i + 1 == n { ";" } else { "" }));
                }
            } else {
                out.push_str(&format!("-- {}\n{sql};\n", s.explanation));
            }
            out.push('\n');
        }
        if self.statements.is_empty() && self.manual.is_empty() {
            out.push_str("-- Nothing to do: the schemas are identical in everything that was compared.\n");
        }
        out
    }
}

fn lc(s: &str) -> String {
    s.to_lowercase()
}

fn find<'a>(s: &'a theta_core::Schema, name: &str) -> Option<&'a Table> {
    s.tables.iter().find(|t| lc(&t.name) == lc(name))
}

/// Make a draft's references to its own schema point at `to`.
fn own_ref(r: Option<&String>, from: &str, to: &str) -> Option<String> {
    match r {
        None => Some(to.to_string()),
        Some(r) if r.eq_ignore_ascii_case(from) => Some(to.to_string()),
        Some(r) => Some(r.clone()),
    }
}

/// `a.` and `` `a`. `` and `"a".` become the same with the target's name; text in between is untouched.
fn requalify(text: &str, from: &str, to: &str) -> String {
    if from.eq_ignore_ascii_case(to) {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (f, t) in [
        (format!("\"{from}\"."), format!("\"{to}\".")),
        (format!("`{from}`."), format!("`{to}`.")),
        (format!("{from}."), format!("{to}.")),
    ] {
        out = replace_ci(&out, &f, &t);
    }
    out
}

fn replace_ci(text: &str, from: &str, to: &str) -> String {
    let (lt, lf) = (text.to_lowercase(), from.to_lowercase());
    if lt.len() != text.len() {
        return text.replace(from, to);
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lt[i..].find(&lf) {
        let at = i + p;
        // Not the tail of a longer identifier (`xa.`).
        let prev = text[..at].chars().next_back();
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') && !from.starts_with(['"', '`']) {
            out.push_str(&text[i..at + from.len()]);
        } else {
            out.push_str(&text[i..at]);
            out.push_str(to);
        }
        i = at + from.len();
    }
    out.push_str(&text[i..]);
    out
}

struct Builder {
    include: bool,
    out: Script,
}

impl Builder {
    fn push(&mut self, object: &str, s: Statement, forced_comment: bool) {
        let destructive = s.is_destructive() && !s.flags.contains(&Flag::Rebuild);
        self.out.statements.push(ScriptStatement {
            sql: s.sql,
            explanation: s.explanation,
            commented: !self.include && (forced_comment || destructive),
            object: object.to_string(),
        });
    }

    fn push_all(&mut self, object: &str, v: Vec<Statement>, forced_comment: bool) {
        for s in v {
            self.push(object, s, forced_comment);
        }
    }

    fn raw(&mut self, object: &str, sql: String, why: String, destructive: bool) {
        self.push(object, Statement::new(sql, why), false);
        if destructive && let Some(s) = self.out.statements.last_mut() {
            s.commented = !self.include;
        }
    }
}

/// The drafts of one table that exists on both sides: the target as it is (`original`) and the source
/// restated in the target's identities (`edited`), so [`ddl::diff`] pairs columns and constraints by name and
/// everything that the comparison calls equal is *literally* equal in the two drafts.
fn pair(
    source: &Table,
    sschema: &str,
    target: &Table,
    tschema: &str,
    kind: DatabaseKind,
    warnings: &mut Vec<String>,
) -> (TableDraft, TableDraft) {
    let mut original = TableDraft::from_table(tschema, target);
    for f in &mut original.foreign_keys {
        f.ref_schema = own_ref(f.ref_schema.as_ref(), tschema, tschema);
    }
    let mut edited = new_table_draft(source, sschema, tschema, &mut Vec::new());
    edited.name = original.name.clone();
    let src_cols = source.columns.clone();
    let mut next = original.next_id();
    let mut fresh = || {
        let v = next;
        next += 1;
        v
    };
    // Old edited ids to new.
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for (c, info) in edited.columns.iter_mut().zip(&src_cols) {
        let old = c.id;
        match original.columns.iter().zip(&target.columns).find(|(o, _)| o.name.eq_ignore_ascii_case(&c.name))
        {
            Some((o, oinfo)) => {
                c.id = o.id;
                let attrs = column_attributes(kind, info, kind, oinfo);
                let has = |a: &str| attrs.iter().any(|x| x == a);
                if !has("type") {
                    c.data_type = o.data_type.clone();
                }
                if !has("nullability") {
                    c.nullable = o.nullable;
                }
                if !has("default") {
                    c.default = o.default.clone();
                } else if is_sequence_default(info.default.as_deref()) {
                    warnings.push(format!(
                        "{}.{}: the source default `{}` uses a sequence; create the sequence in the target and set the default by hand",
                        target.name,
                        c.name,
                        info.default.clone().unwrap_or_default()
                    ));
                    c.default = o.default.clone();
                }
                if !has("identity") {
                    c.identity = o.identity;
                }
                if !has("generated expression") {
                    c.generated = o.generated.clone();
                }
                if !has("comment") {
                    c.comment = o.comment.clone();
                }
            }
            None => c.id = fresh(),
        }
        remap.insert(old, c.id);
    }
    let map_ids = |ids: &[u32]| -> Vec<u32> { ids.iter().filter_map(|i| remap.get(i).copied()).collect() };
    if let Some(pk) = edited.primary_key.as_mut() {
        pk.columns = map_ids(&pk.columns);
        match &original.primary_key {
            // Under the old key's id and name, so a different key is a replacement, never a second key.
            Some(o) => {
                pk.id = o.id;
                pk.name = o.name.clone();
            }
            None => pk.id = fresh(),
        }
    }
    for u in &mut edited.uniques {
        u.columns = map_ids(&u.columns);
        u.id = original
            .uniques
            .iter()
            .find(|o| o.name.as_deref().map(lc) == u.name.as_deref().map(lc))
            .map(|o| o.id)
            .unwrap_or_else(&mut fresh);
    }
    let edited_names: HashMap<u32, String> = edited.columns.iter().map(|c| (c.id, c.name.clone())).collect();
    for f in &mut edited.foreign_keys {
        f.columns = map_ids(&f.columns);
        f.ref_schema = own_ref(f.ref_schema.as_ref(), sschema, tschema);
        let fk_cols = original_cols_of(&original, &f.columns, &edited_names);
        match original.foreign_keys.iter().find(|o| fk_same_key(kind, &original, o, f, &fk_cols)) {
            Some(o) => {
                f.id = o.id;
                let eq = |a: &Option<String>, b: &Option<String>| super::actions_equal(kind, a, b);
                if eq(&f.on_delete, &o.on_delete) {
                    f.on_delete = o.on_delete.clone();
                }
                if eq(&f.on_update, &o.on_update) {
                    f.on_update = o.on_update.clone();
                }
            }
            None => f.id = fresh(),
        }
    }
    for i in &mut edited.indexes {
        i.columns = map_ids(&i.columns);
        match original.indexes.iter().find(|o| lc(&o.name) == lc(&i.name)) {
            Some(o) => {
                i.id = o.id;
                if i.method.as_deref().map(lc).filter(|m| m != "btree")
                    == o.method.as_deref().map(lc).filter(|m| m != "btree")
                {
                    i.method = o.method.clone();
                }
                if i.predicate.as_deref().map(|p| strip_schema(p, sschema))
                    == o.predicate.as_deref().map(|p| strip_schema(p, tschema))
                {
                    i.predicate = o.predicate.clone();
                }
                if let (Some(a), Some(b)) = (&i.raw, &o.raw)
                    && strip_schema(a, sschema) == strip_schema(b, tschema)
                {
                    i.raw = Some(b.clone());
                }
            }
            None => i.id = fresh(),
        }
    }
    for c in &mut edited.checks {
        match original.checks.iter().find(|o| o.name.as_deref().map(lc) == c.name.as_deref().map(lc)) {
            Some(o) => {
                c.id = o.id;
                if super::norm(&c.expression) == super::norm(&o.expression) {
                    c.expression = o.expression.clone();
                }
            }
            None => c.id = fresh(),
        }
    }
    let cm = |c: &Option<String>| c.clone().filter(|c| !c.is_empty());
    if cm(&edited.comment) == cm(&original.comment) {
        edited.comment = original.comment.clone();
    }
    (original, edited)
}

/// The names of the columns a foreign key of `edited` uses (after the ids were mapped to the target's).
fn original_cols_of(_original: &TableDraft, ids: &[u32], edited: &HashMap<u32, String>) -> Vec<String> {
    ids.iter().filter_map(|i| edited.get(i).cloned()).collect()
}

/// Whether the foreign key `f` of the edited draft is the key `o` of the original (see `fk_matches`).
fn fk_same_key(
    kind: DatabaseKind,
    original: &TableDraft,
    o: &ForeignKeyDraft,
    f: &ForeignKeyDraft,
    f_cols: &[String],
) -> bool {
    if kind.family() == Family::Sqlite {
        let o_cols: Vec<String> = o
            .columns
            .iter()
            .filter_map(|i| original.columns.iter().find(|c| c.id == *i))
            .map(|c| c.name.clone())
            .collect();
        super::same_cols(&o_cols, f_cols)
            && o.ref_table.eq_ignore_ascii_case(&f.ref_table)
            && super::same_cols(&o.ref_columns, &f.ref_columns)
    } else {
        o.name.as_deref().map(lc) == f.name.as_deref().map(lc)
    }
}

fn is_sequence_default(d: Option<&str>) -> bool {
    d.and_then(|d| norm_default(Some(d))).is_some_and(|d| d.starts_with("nextval("))
}

/// The draft of a table that is created in the target from the source's description.
fn new_table_draft(source: &Table, sschema: &str, tschema: &str, warnings: &mut Vec<String>) -> TableDraft {
    let mut d = TableDraft::from_table(tschema, source);
    for c in &mut d.columns {
        if is_sequence_default(c.default.as_deref()) {
            warnings.push(format!(
                "{}.{}: the source default `{}` uses a sequence; the column is created without it, create the sequence and set the default by hand",
                source.name,
                c.name,
                c.default.clone().unwrap_or_default()
            ));
            c.default = None;
        }
    }
    for f in &mut d.foreign_keys {
        f.ref_schema = own_ref(f.ref_schema.as_ref(), sschema, tschema);
    }
    for i in &mut d.indexes {
        if let Some(r) = &i.raw {
            i.raw = Some(requalify(r, sschema, tschema));
        }
        if let Some(p) = &i.predicate {
            i.predicate = Some(requalify(p, sschema, tschema));
        }
    }
    d
}

/// The source with every destructive change undone, see the module documentation.
fn held_back(original: &TableDraft, edited: &TableDraft, keep_fks: bool) -> TableDraft {
    let mut e = edited.clone();
    for o in &original.columns {
        if !edited.columns.iter().any(|c| c.id == o.id) {
            e.columns.push(o.clone());
        }
    }
    for c in &mut e.columns {
        if let Some(o) = original.columns.iter().find(|o| o.id == c.id)
            && ddl::narrows(&o.data_type, &c.data_type)
        {
            c.data_type = o.data_type.clone();
        }
    }
    let keep_key =
        |o: &KeyDraft, cur: Option<&KeyDraft>| !cur.is_some_and(|c| c.id == o.id && ddl::same_key(o, c));
    if let Some(o) = &original.primary_key
        && keep_key(o, edited.primary_key.as_ref())
    {
        e.primary_key = Some(o.clone());
    }
    for o in &original.uniques {
        if keep_key(o, edited.uniques.iter().find(|u| u.id == o.id)) {
            e.uniques.retain(|u| u.id != o.id);
            e.uniques.push(o.clone());
        }
    }
    if keep_fks {
        for o in &original.foreign_keys {
            if !edited.foreign_keys.iter().any(|f| f.id == o.id && ddl::same_fk(o, f)) {
                e.foreign_keys.retain(|f: &ForeignKeyDraft| f.id != o.id);
                e.foreign_keys.push(o.clone());
            }
        }
    }
    for o in &original.indexes {
        if !edited.indexes.iter().any(|i| i.id == o.id && ddl::same_index(o, i)) {
            e.indexes.retain(|i: &IndexDraft| i.id != o.id);
            e.indexes.push(o.clone());
        }
    }
    for o in &original.checks {
        let same = |c: &CheckDraft| {
            c.id == o.id && c.name == o.name && super::norm(&c.expression) == super::norm(&o.expression)
        };
        if !edited.checks.iter().any(same) {
            e.checks.retain(|c| c.id != o.id);
            e.checks.push(o.clone());
        }
    }
    e
}

/// Tables ordered so that a table comes after the tables its foreign keys reference (within `tables`).
fn dependency_order<'a>(tables: &[&'a Table]) -> Vec<&'a Table> {
    let mut left: Vec<&Table> = tables.to_vec();
    left.sort_by_key(|t| lc(&t.name));
    let mut out: Vec<&Table> = Vec::new();
    while !left.is_empty() {
        let ready = left.iter().position(|t| {
            t.foreign_keys.iter().all(|f| {
                f.referenced_table.eq_ignore_ascii_case(&t.name)
                    || !left.iter().any(|o| o.name.eq_ignore_ascii_case(&f.referenced_table))
            })
        });
        // A cycle: take the first and let the foreign keys that close it be added afterwards.
        out.push(left.remove(ready.unwrap_or(0)));
    }
    out
}

fn view_statements(
    kind: DatabaseKind,
    name: &str,
    schema: &str,
    def: &str,
    materialized: bool,
    create: bool,
) -> Statement {
    let q =
        qualified_name(Some(schema).filter(|s| !s.is_empty() && kind.family() != Family::Sqlite), name, kind);
    let m = if materialized { "MATERIALIZED " } else { "" };
    if create {
        let body = def.trim().trim_end_matches(';');
        let with = if materialized { " WITH DATA" } else { "" };
        Statement::new(
            format!("CREATE {m}VIEW {q} AS\n{body}{with}"),
            format!("Create {}view {name}", if materialized { "materialized " } else { "" }),
        )
    } else {
        Statement::new(
            format!("DROP {m}VIEW {q}"),
            format!("Drop {}view {name}", if materialized { "materialized " } else { "" }),
        )
        .flag(Flag::Destructive)
    }
}

/// Generate the script. `Err` carries the reason when this pair of schemas cannot have one.
pub fn generate(
    source: Side<'_>,
    target: Side<'_>,
    diff: &SchemaDiff,
    include_destructive: bool,
) -> Result<Script, String> {
    if let Some(why) = diff.script_blocker() {
        return Err(why);
    }
    let kind = target.kind;
    let (sname, tname) = (source.schema.name.as_str(), target.schema.name.as_str());
    let mut b = Builder { include: include_destructive, out: Script::default() };
    let sqlite = kind.family() == Family::Sqlite;
    let table_q = |name: &str| qualified_name(Some(tname).filter(|s| !s.is_empty()), name, kind);

    let of = |k: ObjectKind, st: Status| -> Vec<&super::ObjectDiff> {
        diff.objects.iter().filter(|o| o.kind == k && o.status == st).collect()
    };
    let added: Vec<&Table> = of(ObjectKind::Table, Status::OnlyInSource)
        .iter()
        .filter_map(|o| find(source.schema, &o.name))
        .collect();
    let removed: Vec<&Table> = of(ObjectKind::Table, Status::OnlyInTarget)
        .iter()
        .filter_map(|o| find(target.schema, &o.name))
        .collect();
    let changed: Vec<(&Table, &Table)> = of(ObjectKind::Table, Status::Different)
        .iter()
        .filter_map(|o| Some((find(source.schema, &o.name)?, find(target.schema, &o.name)?)))
        .collect();
    let mut warnings: Vec<String> = Vec::new();

    // Drafts of the changed tables, with a flag for whether anything changed that the diff can express.
    let drafts: Vec<(TableDraft, TableDraft)> =
        changed.iter().map(|(s, t)| pair(s, sname, t, tname, kind, &mut warnings)).collect();

    // 1. Views that go or change.
    let view_kinds = [(ObjectKind::View, false), (ObjectKind::MaterializedView, true)];
    for (vk, mat) in view_kinds {
        for st in [Status::OnlyInTarget, Status::Different] {
            for o in of(vk, st) {
                let destructive = st == Status::OnlyInTarget;
                let s = view_statements(kind, &o.name, tname, "", mat, false);
                let s = if destructive { s } else { Statement { flags: vec![], ..s } };
                b.push(&format!("{} {}", vk.label(), o.name), s, false);
            }
        }
    }

    // 2. Foreign keys that go or change (not SQLite: it rebuilds the table).
    if !sqlite {
        for t in &removed {
            let d = TableDraft::from_table(tname, t);
            for f in &d.foreign_keys {
                b.push(&format!("table {}", t.name), ddl::drop_foreign_key(&d, f, kind), true);
            }
        }
        for (orig, edit) in &drafts {
            for f in &orig.foreign_keys {
                if !edit.foreign_keys.iter().any(|e| e.id == f.id && ddl::same_fk(f, e)) {
                    b.push(&format!("table {}", orig.name), ddl::drop_foreign_key(orig, f, kind), true);
                }
            }
        }
    }

    // 3. Tables that go.
    if !removed.is_empty() {
        let mut order = dependency_order(&removed);
        order.reverse(); // referrers first
        if sqlite {
            b.raw(
                "tables",
                "PRAGMA foreign_keys = OFF".into(),
                "Do not cascade deletes while tables are dropped".into(),
                true,
            );
        }
        for t in order {
            let s = Statement::new(
                format!("DROP TABLE {}", table_q(&t.name)),
                format!("Drop table {} and all of its rows", t.name),
            )
            .flag(Flag::Destructive);
            let s = if ddl::transactional_ddl(kind) { s } else { s.flag(Flag::ImplicitCommit) };
            b.push(&format!("table {}", t.name), s, false);
        }
        if sqlite {
            b.raw(
                "tables",
                "PRAGMA foreign_keys = ON".into(),
                "Turn foreign key enforcement back on".into(),
                true,
            );
        }
    }

    // 4. Tables that appear.
    let new_drafts: Vec<TableDraft> = dependency_order(&added)
        .into_iter()
        .map(|t| new_table_draft(t, sname, tname, &mut warnings))
        .collect();
    for d in &new_drafts {
        let mut bare = d.clone();
        if !sqlite {
            bare.foreign_keys.clear();
        }
        b.push_all(&format!("table {}", d.name), ddl::create_table(&bare, kind), false);
    }

    // 5. Tables that change.
    for (orig, edit) in &drafts {
        let strip = |d: &TableDraft| {
            let mut d = d.clone();
            if !sqlite {
                d.foreign_keys.clear();
            }
            d
        };
        let (o, e) = (strip(orig), strip(edit));
        let object = format!("table {}", orig.name);
        if include_destructive {
            b.push_all(&object, ddl::diff(&o, &e, kind), false);
        } else {
            let safe = held_back(&o, &e, sqlite);
            b.push_all(&object, ddl::diff(&o, &safe, kind), false);
            b.push_all(&object, ddl::diff(&safe, &e, kind), true);
        }
    }

    // 6. Foreign keys that appear or are replaced (not SQLite).
    if !sqlite {
        for d in &new_drafts {
            for f in &d.foreign_keys {
                b.push(&format!("table {}", d.name), ddl::add_foreign_key(d, f, kind), false);
            }
        }
        for (orig, edit) in &drafts {
            for f in &edit.foreign_keys {
                match orig.foreign_keys.iter().find(|o| o.id == f.id) {
                    Some(o) if ddl::same_fk(o, f) => {}
                    // A replaced key can only go back on once the old one is off, which is held back.
                    Some(_) => b.push(
                        &format!("table {}", edit.name),
                        ddl::add_foreign_key(
                            &TableDraft { name: orig.name.clone(), ..edit.clone() },
                            f,
                            kind,
                        ),
                        true,
                    ),
                    None => b.push(
                        &format!("table {}", edit.name),
                        ddl::add_foreign_key(
                            &TableDraft { name: orig.name.clone(), ..edit.clone() },
                            f,
                            kind,
                        ),
                        false,
                    ),
                }
            }
        }
    }

    // 7. Views that appear or change.
    for (vk, mat) in view_kinds {
        let mut wanted: Vec<(&Table, bool)> = Vec::new();
        for o in of(vk, Status::OnlyInSource) {
            if let Some(t) = find(source.schema, &o.name) {
                wanted.push((t, false));
            }
        }
        for o in of(vk, Status::Different) {
            if let Some(t) = find(source.schema, &o.name) {
                wanted.push((t, true));
            }
        }
        wanted.sort_by_key(|(t, _)| lc(&t.name));
        // A view that mentions another wanted view goes after it.
        let mut ordered: Vec<(&Table, bool)> = Vec::new();
        while !wanted.is_empty() {
            let i = wanted
                .iter()
                .position(|(t, _)| {
                    let def = t.view_definition.clone().unwrap_or_default().to_lowercase();
                    !wanted.iter().any(|(o, _)| o.name != t.name && mentions_name(&def, &o.name))
                })
                .unwrap_or(0);
            ordered.push(wanted.remove(i));
        }
        for (t, _) in ordered {
            match &t.view_definition {
                Some(def) => {
                    let def = requalify(def, sname, tname);
                    b.push(
                        &format!("{} {}", vk.label(), t.name),
                        view_statements(kind, &t.name, tname, &def, mat, true),
                        false,
                    );
                    if !sname.eq_ignore_ascii_case(tname) {
                        warnings.push(format!("view {}: its text was copied from schema {sname} with qualified references renamed to {tname}; names without a schema resolve through the search path, check them", t.name));
                    }
                }
                None => warnings
                    .push(format!("view {}: its definition could not be read, so it is not created", t.name)),
            }
        }
    }

    // What the script cannot do.
    for o in &diff.objects {
        match (o.kind, o.status) {
            (ObjectKind::Routine, Status::OnlyInSource) => warnings.push(format!("routine {} exists only in the source; the catalog has no routine body, so it cannot be created", o.name)),
            (ObjectKind::Routine, Status::OnlyInTarget) => warnings.push(format!("routine {} exists only in the target; it is not dropped by this script", o.name)),
            (ObjectKind::Routine, Status::Different) => warnings.push(format!("routine {} differs in kind or return type; change it by hand", o.name)),
            (ObjectKind::Table, Status::Different) => {
                for c in &o.changes {
                    if c.element == super::Element::TableOption {
                        warnings.push(format!("table {}: option {} differs ({} in the source, {} in the target); an ALTER could rewrite the table, so it is left to you", o.name, c.name, c.source.clone().unwrap_or_default(), c.target.clone().unwrap_or_default()));
                    }
                }
            }
            _ => {}
        }
    }
    warnings.sort();
    warnings.dedup();
    b.out.manual = warnings;
    Ok(b.out)
}

fn mentions_name(text: &str, name: &str) -> bool {
    let n = name.to_lowercase();
    let mut from = 0;
    while let Some(p) = text[from..].find(&n) {
        let at = from + p;
        let ok = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        if ok(text[..at].chars().next_back()) && ok(text[at + n.len()..].chars().next()) {
            return true;
        }
        from = at + n.len();
    }
    false
}

#[cfg(test)]
pub(super) fn requalify_for_test(text: &str, from: &str, to: &str) -> String {
    requalify(text, from, to)
}
