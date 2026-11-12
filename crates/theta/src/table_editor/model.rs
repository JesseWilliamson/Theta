//! The table editor's state without a window: the draft being edited, what it was, undo and redo, the
//! rows each section shows, and the script the edit amounts to.
//!
//! The view only paints a [`Spec`] and forwards what the user did as a [`CellValue`] or an operation on
//! this model. Everything that decides what an edit *means* (a primary key column becomes NOT NULL,
//! deleting a column takes its constraints with it, a new id never reuses one of a removed column) lives
//! here, where it is tested without GPUI. What the edit costs on the server is the DDL engine's business
//! (`theta_sql::ddl`): every change re-runs `diff` / `create_table` and `validate` and the model keeps the
//! result as its [`Analysis`].

use std::sync::Arc;
use theta_core::Family;
use theta_core::{Catalog, DatabaseKind, Identity, Table};
use theta_sql::ddl::{
    self, CheckDraft, ColumnDraft, Fix, ForeignKeyDraft, IndexDraft, Issue, KeyDraft, Scope, Severity,
    Statement, TableDraft,
};

/// The steps back the user can take.
const UNDO_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Columns,
    Keys,
    ForeignKeys,
    Indexes,
    Checks,
    Options,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Columns,
        Section::Keys,
        Section::ForeignKeys,
        Section::Indexes,
        Section::Checks,
        Section::Options,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Columns => "Columns",
            Section::Keys => "Keys",
            Section::ForeignKeys => "Foreign Keys",
            Section::Indexes => "Indexes",
            Section::Checks => "Checks",
            Section::Options => "Options",
        }
    }

    pub(crate) fn scope_of(self) -> Scope {
        self.scope()
    }

    /// The validation scope that belongs to this section.
    fn scope(self) -> Scope {
        match self {
            Section::Columns => Scope::Columns,
            Section::Keys => Scope::Keys,
            Section::ForeignKeys => Scope::ForeignKeys,
            Section::Indexes => Scope::Indexes,
            Section::Checks => Scope::Checks,
            Section::Options => Scope::Table,
        }
    }
}

/// Which row of a section. Ids are the draft's own, so a row keeps its identity while its neighbours move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowId {
    Column(u32),
    /// A column of the original table that the edit drops; shown struck through until Execute.
    Removed(u32),
    Pk,
    Unique(u32),
    Fk(u32),
    Index(u32),
    Check(u32),
    Comment,
    /// A read-only storage option (MySQL engine, charset, collation) by position.
    Info(usize),
}

/// One editable property of a row; a section's header lists the fields it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Type,
    Nullable,
    Default,
    Pk,
    Identity,
    Comment,
    KeyName,
    KeyKind,
    KeyColumns,
    FkName,
    FkColumns,
    FkRefTable,
    FkRefColumns,
    FkOnDelete,
    FkOnUpdate,
    FkDeferrable,
    IdxName,
    IdxColumns,
    IdxUnique,
    IdxMethod,
    IdxPredicate,
    CheckName,
    CheckExpr,
    OptName,
    OptValue,
}

/// How a cell behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// Free text, edited in place.
    Text,
    /// Free text with the dialect's type suggestions.
    Type,
    /// A checkbox.
    Toggle,
    /// One of a few words; Space steps through them.
    Cycle,
    /// An ordered list of columns, edited in a small popup.
    Columns,
    /// The referenced table of a foreign key, chosen from the catalog.
    RefTable,
    /// Read-only.
    Static,
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub field: Field,
    pub kind: CellKind,
    pub text: String,
    pub on: bool,
    pub chips: Vec<String>,
    /// Drawn dim: a placeholder such as "(auto)" or a value that cannot be changed.
    pub muted: bool,
}

impl Cell {
    fn new(field: Field, kind: CellKind) -> Self {
        Self { field, kind, text: String::new(), on: false, chips: Vec::new(), muted: false }
    }
    fn text(field: Field, kind: CellKind, text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::new(field, kind) }
    }
    fn placeholder(mut self, text: &str) -> Self {
        if self.text.is_empty() {
            self.text = text.into();
            self.muted = true;
        }
        self
    }
    fn dim(mut self) -> Self {
        self.muted = true;
        self
    }
    fn toggle(field: Field, on: bool) -> Self {
        Self { on, ..Self::new(field, CellKind::Toggle) }
    }
    fn chips(field: Field, kind: CellKind, chips: Vec<String>) -> Self {
        Self { chips, ..Self::new(field, kind) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Unchanged,
    Added,
    Modified,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub id: RowId,
    pub cells: Vec<Cell>,
    pub struck: bool,
    pub state: RowState,
}

#[derive(Debug, Clone)]
pub struct ColSpec {
    pub field: Field,
    pub title: &'static str,
    /// Pixels; 0 means "share the rest".
    pub width: f32,
}

/// A section as a table of cells.
#[derive(Debug, Clone)]
pub struct Spec {
    pub cols: Vec<ColSpec>,
    pub rows: Vec<Row>,
}

impl Spec {
    pub fn index_of(&self, id: RowId) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }
}

/// What the user entered into a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Text(String),
    /// Toggle a checkbox.
    Flip,
    /// Step a [`CellKind::Cycle`] cell forward (`1`) or back (`-1`).
    Step(i32),
}

/// What the current draft amounts to.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub statements: Vec<Statement>,
    pub issues: Vec<Issue>,
    pub summary: String,
}

impl Analysis {
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == Severity::Error).count()
    }

    /// The worst problem in a section, for its badge.
    pub fn worst(&self, section: Section) -> Option<Severity> {
        let scope = section.scope();
        let mut worst = None;
        // A problem with the name is shown at the name box and in the preview; it must not make the Options
        // tab carry a red dot nobody can explain.
        let about_name = |i: &Issue| {
            section == Section::Options
                && (i.message.starts_with("The table needs a name") || i.message.contains("already exists"))
        };
        for i in self.issues.iter().filter(|i| i.scope == scope && !about_name(i)) {
            match i.severity {
                Severity::Error => return Some(Severity::Error),
                Severity::Warning => worst = Some(Severity::Warning),
            }
        }
        worst
    }
}

pub struct DraftModel {
    kind: DatabaseKind,
    /// `None` for a table that does not exist yet.
    original: Option<TableDraft>,
    /// What "no changes" means: the original, or the starter draft of a new table.
    baseline: TableDraft,
    draft: TableDraft,
    catalog: Option<Arc<Catalog>>,
    /// MySQL storage options shown read-only.
    info: Vec<(String, String)>,
    undo: Vec<TableDraft>,
    redo: Vec<TableDraft>,
    /// Never reused, so a new column cannot take the id of one the edit removed (the diff pairs by id).
    next_id: u32,
    analysis: Analysis,
}

impl DraftModel {
    /// An editor for a table that is about to be created. It starts with an `id` key column, as most
    /// tables want one; deleting it is one keystroke.
    pub fn new_table(kind: DatabaseKind, schema: Option<String>, catalog: Option<Arc<Catalog>>) -> Self {
        let schema = schema.filter(|_| kind.family() != Family::Sqlite);
        let int = match kind.family() {
            Family::MySql => "int",
            Family::Sqlite => "INTEGER",
            // The other families' kinds have the table editor disabled (`DatabaseKind::unsupported`);
            // PostgreSQL's vocabulary keeps this total.
            Family::Postgres | Family::DuckDb | Family::SqlServer | Family::ClickHouse => "integer",
        };
        let draft = TableDraft {
            schema,
            name: String::new(),
            columns: vec![ColumnDraft {
                id: 1,
                name: "id".into(),
                data_type: int.into(),
                nullable: false,
                default: None,
                identity: Some(Identity::ByDefault),
                generated: None,
                comment: None,
            }],
            primary_key: Some(KeyDraft { id: 2, name: None, columns: vec![1] }),
            ..Default::default()
        };
        Self::build(kind, None, draft, catalog, Vec::new())
    }

    /// An editor for an existing table.
    pub fn modify(kind: DatabaseKind, schema: &str, table: &Table, catalog: Option<Arc<Catalog>>) -> Self {
        let mut original = TableDraft::from_table(schema, table);
        // SQLite's own database is always `main`; naming it only clutters the generated SQL.
        if kind.family() == Family::Sqlite {
            if schema.eq_ignore_ascii_case("main") {
                original.schema = None;
            }
            forget_invented_names(&mut original);
        }
        let info = table.options.iter().map(|o| (o.name.clone(), o.value.clone())).collect();
        Self::build(kind, Some(original.clone()), original, catalog, info)
    }

    fn build(
        kind: DatabaseKind,
        original: Option<TableDraft>,
        draft: TableDraft,
        catalog: Option<Arc<Catalog>>,
        info: Vec<(String, String)>,
    ) -> Self {
        let next_id = draft.next_id().max(original.as_ref().map_or(0, TableDraft::next_id));
        let mut m = Self {
            kind,
            baseline: draft.clone(),
            original,
            draft,
            catalog,
            info,
            undo: Vec::new(),
            redo: Vec::new(),
            next_id,
            analysis: Analysis::default(),
        };
        m.reanalyze();
        m
    }

    // ------------------------------------------------------------- reading

    pub fn kind(&self) -> DatabaseKind {
        self.kind
    }
    pub fn is_new(&self) -> bool {
        self.original.is_none()
    }
    pub fn draft(&self) -> &TableDraft {
        &self.draft
    }
    pub fn original(&self) -> Option<&TableDraft> {
        self.original.as_ref()
    }
    pub fn analysis(&self) -> &Analysis {
        &self.analysis
    }
    pub fn dirty(&self) -> bool {
        self.draft != self.baseline
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        self.catalog.as_ref()
    }

    pub fn set_catalog(&mut self, catalog: Option<Arc<Catalog>>) {
        self.catalog = catalog;
        self.reanalyze();
    }

    /// The number shown on a section's badge.
    pub fn count(&self, section: Section) -> usize {
        let d = &self.draft;
        match section {
            Section::Columns => d.columns.len(),
            Section::Keys => d.uniques.len() + usize::from(d.primary_key.is_some()),
            Section::ForeignKeys => d.foreign_keys.len(),
            Section::Indexes => d.indexes.len(),
            Section::Checks => d.checks.len(),
            Section::Options => usize::from(d.comment.is_some()),
        }
    }

    fn reanalyze(&mut self) {
        let mut issues = ddl::validate(&self.draft, self.kind);
        if let Some(catalog) = &self.catalog {
            issues.extend(ddl::check_foreign_keys(&self.draft, catalog));
            issues.extend(ddl::check_names(
                &self.draft,
                self.original.as_ref().map(|o| o.name.as_str()),
                catalog,
                self.kind,
            ));
        }
        if let Some(o) = &self.original {
            issues.extend(ddl::check_changes(o, &self.draft, self.catalog.as_deref(), self.kind));
        }
        let statements = match &self.original {
            Some(o) => ddl::diff(o, &self.draft, self.kind),
            None => ddl::create_table(&self.draft, self.kind),
        };
        let summary = if self.dirty() { ddl::summarize(&statements) } else { "No changes".into() };
        // Nothing to run when nothing changed, even though a new table's starter draft would render.
        let statements = if self.dirty() { statements } else { Vec::new() };
        self.analysis = Analysis { statements, issues, summary };
    }

    // ------------------------------------------------------------- editing

    /// Run `f` on the draft as one undoable step. An error or an edit that changes nothing leaves no step.
    fn edit<R>(
        &mut self,
        f: impl FnOnce(&mut TableDraft, &mut u32, DatabaseKind) -> Result<R, String>,
    ) -> Result<R, String> {
        let before = self.draft.clone();
        let result = f(&mut self.draft, &mut self.next_id, self.kind);
        match result {
            Err(e) => {
                self.draft = before;
                Err(e)
            }
            Ok(r) => {
                if self.draft != before {
                    self.undo.push(before);
                    if self.undo.len() > UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.reanalyze();
                }
                Ok(r)
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.draft, prev));
        self.reanalyze();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.draft, next));
        self.reanalyze();
        true
    }

    /// Throw every edit away (one undoable step, so Revert can itself be undone).
    pub fn revert(&mut self) {
        let base = self.baseline.clone();
        let _ = self.edit(|d, _, _| {
            *d = base;
            Ok(())
        });
    }

    /// The `CREATE TRIGGER` statements of a SQLite table. A table rebuild drops the triggers with the old table,
    /// so the engine replays these afterwards; the catalog does not carry them, the workspace reads them from
    /// `sqlite_master` and hands them over. Not an edit: the original and the baseline get them too.
    pub fn set_recreate(&mut self, triggers: Vec<String>) {
        if let Some(o) = &mut self.original {
            o.recreate = triggers.clone();
        }
        self.baseline.recreate = triggers.clone();
        self.draft.recreate = triggers;
        self.undo
            .iter_mut()
            .chain(self.redo.iter_mut())
            .for_each(|d| d.recreate = self.draft.recreate.clone());
        self.reanalyze();
    }

    /// What was just applied becomes the original: the diff is empty again and undo starts afresh. Used when
    /// the database cannot be asked what the table looks like now (the change sits in an open transaction).
    pub fn accept(&mut self) {
        self.original = Some(self.draft.clone());
        self.baseline = self.draft.clone();
        self.undo.clear();
        self.redo.clear();
        self.reanalyze();
    }

    pub fn apply_fix(&mut self, fix: &Fix) {
        let fix = fix.clone();
        let _ = self.edit(|d, _, _| {
            ddl::apply_fix(d, &fix);
            Ok(())
        });
    }

    pub fn set_table_name(&mut self, name: &str) {
        let name = name.trim().to_string();
        let _ = self.edit(|d, _, _| {
            d.name = name;
            Ok(())
        });
    }

    pub fn set_schema(&mut self, schema: Option<String>) {
        let _ = self.edit(|d, _, _| {
            d.schema = schema;
            Ok(())
        });
    }

    /// Set a cell. `Err` is a sentence for the user (the cell cannot take this value).
    pub fn set_cell(&mut self, row: RowId, field: Field, value: CellValue) -> Result<(), String> {
        self.edit(|d, fresh, kind| set_cell(d, fresh, kind, row, field, value))
    }

    /// Replace the column list of a key, foreign key or index row.
    pub fn set_columns(&mut self, row: RowId, ids: Vec<u32>) {
        let _ = self.edit(|d, fresh, _| {
            match row {
                RowId::Pk => {
                    if ids.is_empty() {
                        d.primary_key = None;
                    } else {
                        let key = d.primary_key.get_or_insert_with(|| KeyDraft {
                            id: bump(fresh),
                            name: None,
                            columns: vec![],
                        });
                        key.columns = ids.clone();
                        for id in &ids {
                            if let Some(c) = d.columns.iter_mut().find(|c| c.id == *id) {
                                c.nullable = false;
                            }
                        }
                    }
                }
                RowId::Unique(id) => {
                    if let Some(k) = d.uniques.iter_mut().find(|k| k.id == id) {
                        k.columns = ids;
                    }
                }
                RowId::Fk(id) => {
                    if let Some(f) = d.foreign_keys.iter_mut().find(|f| f.id == id) {
                        f.columns = ids;
                    }
                }
                RowId::Index(id) => {
                    let table = d.name.clone();
                    let names = |d: &TableDraft, ids: &[u32]| -> Vec<String> {
                        ids.iter()
                            .filter_map(|i| d.columns.iter().find(|c| c.id == *i))
                            .map(|c| c.name.clone())
                            .collect()
                    };
                    let old = d
                        .indexes
                        .iter()
                        .find(|i| i.id == id)
                        .map(|i| auto_index_name(&table, &names(d, &i.columns)));
                    let new_name = auto_index_name(&table, &names(d, &ids));
                    if let Some(i) = d.indexes.iter_mut().find(|i| i.id == id) {
                        // A name the user never typed follows the columns.
                        if i.name.is_empty() || old.as_deref() == Some(i.name.as_str()) {
                            i.name = new_name;
                        }
                        i.columns = ids;
                    }
                }
                _ => {}
            }
            Ok(())
        });
    }

    /// Replace the referenced columns of a foreign key.
    pub fn set_ref_columns(&mut self, fk: u32, columns: Vec<String>) {
        let _ = self.edit(|d, _, _| {
            if let Some(f) = d.foreign_keys.iter_mut().find(|f| f.id == fk) {
                f.ref_columns = columns;
            }
            Ok(())
        });
    }

    /// Point a foreign key at `schema.table`. The referenced columns default to that table's primary
    /// key, which is what nearly every foreign key means.
    pub fn set_fk_target(&mut self, fk: u32, schema: Option<String>, table: &str) {
        let pk = self
            .catalog
            .as_ref()
            .and_then(|c| c.find_table(schema.as_deref(), table))
            .map(|(_, t)| t.primary_key.clone());
        let table = table.to_string();
        let _ = self.edit(|d, _, _| {
            if let Some(f) = d.foreign_keys.iter_mut().find(|f| f.id == fk) {
                let changed = f.ref_table != table || f.ref_schema != schema;
                f.ref_schema = schema;
                f.ref_table = table;
                if changed {
                    f.ref_columns = pk.unwrap_or_default();
                }
            }
            Ok(())
        });
    }

    /// Add a row after (or, with `above`, before) `anchor`; at the end without one. `None` where the
    /// section has nothing to add (Options).
    pub fn add_row(&mut self, section: Section, anchor: Option<RowId>, above: bool) -> Option<RowId> {
        let kind = self.kind;
        self.edit(|d, fresh, _| {
            let id = bump(fresh);
            Ok(match section {
                Section::Columns => {
                    let at = match anchor {
                        Some(RowId::Column(a)) => {
                            let p = d.columns.iter().position(|c| c.id == a).unwrap_or(d.columns.len());
                            if above { p } else { p + 1 }
                        }
                        // Removed rows sit between live ones: new columns go after the last live one.
                        _ => d.columns.len(),
                    };
                    let at = if at > d.columns.len() { d.columns.len() } else { at };
                    d.columns.insert(
                        at,
                        ColumnDraft {
                            id,
                            name: String::new(),
                            data_type: match kind.family() {
                                Family::MySql => "varchar(255)",
                                Family::Sqlite => "TEXT",
                                _ => "text",
                            }
                            .into(),
                            nullable: true,
                            default: None,
                            identity: None,
                            generated: None,
                            comment: None,
                        },
                    );
                    Some(RowId::Column(id))
                }
                Section::Keys => {
                    d.uniques.push(KeyDraft { id, name: None, columns: vec![] });
                    Some(RowId::Unique(id))
                }
                Section::ForeignKeys => {
                    d.foreign_keys.push(ForeignKeyDraft {
                        id,
                        name: None,
                        columns: vec![],
                        ref_schema: None,
                        ref_table: String::new(),
                        ref_columns: vec![],
                        on_delete: None,
                        on_update: None,
                        deferrable: false,
                        initially_deferred: false,
                    });
                    Some(RowId::Fk(id))
                }
                Section::Indexes => {
                    d.indexes.push(IndexDraft {
                        id,
                        name: String::new(),
                        columns: vec![],
                        unique: false,
                        method: None,
                        predicate: None,
                        raw: None,
                    });
                    Some(RowId::Index(id))
                }
                Section::Checks => {
                    d.checks.push(CheckDraft { id, name: None, expression: String::new() });
                    Some(RowId::Check(id))
                }
                Section::Options => None,
            })
        })
        .ok()
        .flatten()
    }

    /// Delete a row. A column of an existing table stays visible, struck through ([`RowId::Removed`]);
    /// deleting that row again brings it back.
    pub fn delete_row(&mut self, row: RowId) -> Result<(), String> {
        match row {
            RowId::Removed(id) => {
                self.restore_column(id);
                Ok(())
            }
            RowId::Column(id) => self.edit(|d, _, _| {
                remove_column(d, id);
                Ok(())
            }),
            RowId::Pk => self.edit(|d, _, _| {
                d.primary_key = None;
                Ok(())
            }),
            RowId::Unique(id) => self.edit(|d, _, _| {
                d.uniques.retain(|k| k.id != id);
                Ok(())
            }),
            RowId::Fk(id) => self.edit(|d, _, _| {
                d.foreign_keys.retain(|k| k.id != id);
                Ok(())
            }),
            RowId::Index(id) => self.edit(|d, _, _| {
                d.indexes.retain(|k| k.id != id);
                Ok(())
            }),
            RowId::Check(id) => self.edit(|d, _, _| {
                d.checks.retain(|k| k.id != id);
                Ok(())
            }),
            RowId::Comment => self.edit(|d, _, _| {
                d.comment = None;
                Ok(())
            }),
            RowId::Info(_) => Err("This option is read-only here".into()),
        }
    }

    /// Bring back a removed column and, where nothing else changed about them, the constraints that went
    /// with it.
    pub fn restore_column(&mut self, id: u32) {
        let Some(orig) = self.original.clone() else { return };
        let _ = self.edit(|d, _, _| {
            let Some(col) = orig.columns.iter().find(|c| c.id == id).cloned() else { return Ok(()) };
            if d.columns.iter().any(|c| c.id == id) {
                return Ok(());
            }
            // Put it after the nearest earlier original column that is still here.
            let pos = orig.columns.iter().position(|c| c.id == id).unwrap_or(0);
            let at = orig.columns[..pos]
                .iter()
                .rev()
                .find_map(|o| d.columns.iter().position(|c| c.id == o.id).map(|p| p + 1))
                .unwrap_or(0);
            d.columns.insert(at, col);
            let present =
                |d: &TableDraft, ids: &[u32]| ids.iter().all(|i| d.columns.iter().any(|c| c.id == *i));
            if let Some(opk) = &orig.primary_key
                && opk.columns.contains(&id)
                && d.primary_key.as_ref().is_none_or(|k| k.id == opk.id)
            {
                let cols: Vec<u32> =
                    opk.columns.iter().copied().filter(|i| d.columns.iter().any(|c| c.id == *i)).collect();
                d.primary_key = Some(KeyDraft { columns: cols, ..opk.clone() });
            }
            for u in orig.uniques.iter().filter(|u| u.columns.contains(&id)) {
                if !d.uniques.iter().any(|k| k.id == u.id) && present(d, &u.columns) {
                    d.uniques.push(u.clone());
                }
            }
            for f in orig.foreign_keys.iter().filter(|f| f.columns.contains(&id)) {
                if !d.foreign_keys.iter().any(|k| k.id == f.id) && present(d, &f.columns) {
                    d.foreign_keys.push(f.clone());
                }
            }
            for i in orig.indexes.iter().filter(|i| i.columns.contains(&id)) {
                if !present(d, &i.columns) {
                    continue;
                }
                match d.indexes.iter().position(|k| k.id == i.id) {
                    None => d.indexes.push(i.clone()),
                    // The index lost this column when it was removed: it gets it back at its old place.
                    Some(p) => d.indexes[p].columns = i.columns.clone(),
                }
            }
            Ok(())
        });
    }

    /// Move a column one place up or down. Only a table that does not exist yet has an order to change:
    /// no dialect's `ALTER TABLE` reorders columns (MySQL's `AFTER` is not generated), so for an existing
    /// table the move would be a lie.
    pub fn move_column(&mut self, id: u32, delta: i32) -> Result<bool, String> {
        if self.original.is_some() {
            return Err("The column order of an existing table cannot be changed".into());
        }
        self.edit(|d, _, _| {
            let Some(p) = d.columns.iter().position(|c| c.id == id) else { return Ok(false) };
            let to = p as i32 + delta;
            if to < 0 || to as usize >= d.columns.len() {
                return Ok(false);
            }
            d.columns.swap(p, to as usize);
            Ok(true)
        })
    }

    /// Copy a column (not its key membership) right below itself.
    pub fn duplicate_column(&mut self, id: u32) -> Option<RowId> {
        self.edit(|d, fresh, _| {
            let Some(p) = d.columns.iter().position(|c| c.id == id) else { return Ok(None) };
            let mut c = d.columns[p].clone();
            c.id = bump(fresh);
            c.name = format!("{}_copy", c.name);
            // An identity column twice is rarely wanted, and the copy would fight for the same key.
            c.identity = None;
            let new = c.id;
            d.columns.insert(p + 1, c);
            Ok(Some(RowId::Column(new)))
        })
        .ok()
        .flatten()
    }

    // ------------------------------------------------------------ the rows

    /// The ids of the columns that are still in the draft, in order, with their names.
    pub fn column_choices(&self) -> Vec<(u32, String)> {
        self.draft.columns.iter().map(|c| (c.id, c.name.clone())).collect()
    }

    pub fn column_name(&self, id: u32) -> String {
        self.draft.columns.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default()
    }

    fn names(&self, ids: &[u32]) -> Vec<String> {
        ids.iter().map(|i| self.column_name(*i)).collect()
    }

    pub fn spec(&self, section: Section) -> Spec {
        let kind = self.kind;
        match section {
            Section::Columns => self.columns_spec(),
            Section::Keys => {
                let cols = vec![
                    ColSpec { field: Field::KeyKind, title: "Constraint", width: 130.0 },
                    ColSpec { field: Field::KeyName, title: "Name", width: 220.0 },
                    ColSpec { field: Field::KeyColumns, title: "Columns", width: 0.0 },
                ];
                let mut rows = Vec::new();
                let pk = self.draft.primary_key.as_ref();
                let pk_name = match (kind.family(), pk) {
                    (Family::MySql, _) => Cell::text(Field::KeyName, CellKind::Static, "PRIMARY").dim(),
                    (_, Some(k)) => {
                        Cell::text(Field::KeyName, CellKind::Text, k.name.clone().unwrap_or_default())
                            .placeholder("(auto)")
                    }
                    _ => Cell::text(Field::KeyName, CellKind::Text, "").placeholder("(auto)"),
                };
                rows.push(Row {
                    id: RowId::Pk,
                    cells: vec![
                        Cell::text(Field::KeyKind, CellKind::Static, "Primary key"),
                        pk_name,
                        Cell::chips(
                            Field::KeyColumns,
                            CellKind::Columns,
                            self.names(pk.map_or(&[], |k| &k.columns)),
                        ),
                    ],
                    struck: false,
                    state: self.key_state(pk, self.original.as_ref().and_then(|o| o.primary_key.as_ref())),
                });
                for u in &self.draft.uniques {
                    let orig = self.original.as_ref().and_then(|o| o.uniques.iter().find(|k| k.id == u.id));
                    rows.push(Row {
                        id: RowId::Unique(u.id),
                        cells: vec![
                            Cell::text(Field::KeyKind, CellKind::Static, "Unique"),
                            Cell::text(Field::KeyName, CellKind::Text, u.name.clone().unwrap_or_default())
                                .placeholder("(auto)"),
                            Cell::chips(Field::KeyColumns, CellKind::Columns, self.names(&u.columns)),
                        ],
                        struck: false,
                        state: self.key_state(Some(u), orig),
                    });
                }
                Spec { cols, rows }
            }
            Section::ForeignKeys => {
                let deferrable = ddl::supports_deferrable(kind);
                let mut cols = vec![
                    ColSpec { field: Field::FkName, title: "Name", width: 190.0 },
                    ColSpec { field: Field::FkColumns, title: "Columns", width: 170.0 },
                    ColSpec { field: Field::FkRefTable, title: "References", width: 190.0 },
                    ColSpec { field: Field::FkRefColumns, title: "Referenced columns", width: 170.0 },
                    ColSpec { field: Field::FkOnDelete, title: "On delete", width: 110.0 },
                    ColSpec { field: Field::FkOnUpdate, title: "On update", width: 110.0 },
                ];
                if deferrable {
                    cols.push(ColSpec { field: Field::FkDeferrable, title: "Deferrable", width: 80.0 });
                }
                let rows = self
                    .draft
                    .foreign_keys
                    .iter()
                    .map(|f| {
                        let orig =
                            self.original.as_ref().and_then(|o| o.foreign_keys.iter().find(|k| k.id == f.id));
                        let target = if f.ref_table.is_empty() {
                            String::new()
                        } else {
                            match &f.ref_schema {
                                Some(s) if kind.family() != Family::Sqlite => format!("{s}.{}", f.ref_table),
                                _ => f.ref_table.clone(),
                            }
                        };
                        let action = |v: &Option<String>| v.clone().unwrap_or_else(|| "NO ACTION".into());
                        let mut cells = vec![
                            Cell::text(Field::FkName, CellKind::Text, f.name.clone().unwrap_or_default())
                                .placeholder("(auto)"),
                            Cell::chips(Field::FkColumns, CellKind::Columns, self.names(&f.columns)),
                            Cell::text(Field::FkRefTable, CellKind::RefTable, target),
                            Cell::chips(Field::FkRefColumns, CellKind::Columns, f.ref_columns.clone()),
                            Cell::text(Field::FkOnDelete, CellKind::Cycle, action(&f.on_delete)),
                            Cell::text(Field::FkOnUpdate, CellKind::Cycle, action(&f.on_update)),
                        ];
                        if deferrable {
                            cells.push(Cell::toggle(Field::FkDeferrable, f.deferrable));
                        }
                        Row {
                            id: RowId::Fk(f.id),
                            cells,
                            struck: false,
                            state: state_of(orig.map(|o| o == f)),
                        }
                    })
                    .collect();
                Spec { cols, rows }
            }
            Section::Indexes => {
                let mut cols = vec![
                    ColSpec { field: Field::IdxName, title: "Name", width: 230.0 },
                    ColSpec { field: Field::IdxColumns, title: "Columns", width: 0.0 },
                    ColSpec { field: Field::IdxUnique, title: "Unique", width: 64.0 },
                ];
                if !ddl::index_methods(kind).is_empty() {
                    cols.push(ColSpec { field: Field::IdxMethod, title: "Method", width: 90.0 });
                }
                if ddl::supports_partial_indexes(kind) {
                    cols.push(ColSpec { field: Field::IdxPredicate, title: "Where", width: 200.0 });
                }
                let rows = self
                    .draft
                    .indexes
                    .iter()
                    .map(|i| {
                        let orig =
                            self.original.as_ref().and_then(|o| o.indexes.iter().find(|k| k.id == i.id));
                        let chips = match &i.raw {
                            // An expression index cannot be authored here; its definition is shown as it is.
                            Some(raw) => vec![index_expression_text(raw)],
                            None => self.names(&i.columns),
                        };
                        let columns_kind = if i.raw.is_some() { CellKind::Static } else { CellKind::Columns };
                        let mut cells = vec![
                            Cell::text(Field::IdxName, CellKind::Text, i.name.clone()).placeholder("(name)"),
                            Cell::chips(Field::IdxColumns, columns_kind, chips),
                            Cell::toggle(Field::IdxUnique, i.unique),
                        ];
                        if !ddl::index_methods(kind).is_empty() {
                            cells.push(Cell::text(
                                Field::IdxMethod,
                                CellKind::Cycle,
                                i.method.clone().unwrap_or_else(|| ddl::index_methods(kind)[0].to_string()),
                            ));
                        }
                        if ddl::supports_partial_indexes(kind) {
                            cells.push(
                                Cell::text(
                                    Field::IdxPredicate,
                                    CellKind::Text,
                                    i.predicate.clone().unwrap_or_default(),
                                )
                                .placeholder("(all rows)"),
                            );
                        }
                        Row {
                            id: RowId::Index(i.id),
                            cells,
                            struck: false,
                            state: state_of(orig.map(|o| o == i)),
                        }
                    })
                    .collect();
                Spec { cols, rows }
            }
            Section::Checks => {
                let cols = vec![
                    ColSpec { field: Field::CheckName, title: "Name", width: 230.0 },
                    ColSpec { field: Field::CheckExpr, title: "Expression", width: 0.0 },
                ];
                let rows = self
                    .draft
                    .checks
                    .iter()
                    .map(|c| {
                        let orig =
                            self.original.as_ref().and_then(|o| o.checks.iter().find(|k| k.id == c.id));
                        Row {
                            id: RowId::Check(c.id),
                            cells: vec![
                                Cell::text(
                                    Field::CheckName,
                                    CellKind::Text,
                                    c.name.clone().unwrap_or_default(),
                                )
                                .placeholder("(auto)"),
                                Cell::text(Field::CheckExpr, CellKind::Text, c.expression.clone())
                                    .placeholder("e.g. price > 0"),
                            ],
                            struck: false,
                            state: state_of(orig.map(|o| o == c)),
                        }
                    })
                    .collect();
                Spec { cols, rows }
            }
            Section::Options => {
                let cols = vec![
                    ColSpec { field: Field::OptName, title: "Option", width: 200.0 },
                    ColSpec { field: Field::OptValue, title: "Value", width: 0.0 },
                ];
                let mut rows = Vec::new();
                if ddl::supports_table_comment(kind) {
                    let changed = self.draft.comment != self.baseline.comment;
                    rows.push(Row {
                        id: RowId::Comment,
                        cells: vec![
                            Cell::text(Field::OptName, CellKind::Static, "Table comment"),
                            Cell::text(
                                Field::OptValue,
                                CellKind::Text,
                                self.draft.comment.clone().unwrap_or_default(),
                            )
                            .placeholder("(none)"),
                        ],
                        struck: false,
                        state: if changed && !self.is_new() {
                            RowState::Modified
                        } else {
                            RowState::Unchanged
                        },
                    });
                }
                for (n, (name, value)) in self.info.iter().enumerate() {
                    rows.push(Row {
                        id: RowId::Info(n),
                        cells: vec![
                            Cell::text(Field::OptName, CellKind::Static, name.clone()),
                            Cell::text(Field::OptValue, CellKind::Static, value.clone()).dim(),
                        ],
                        struck: false,
                        state: RowState::Unchanged,
                    });
                }
                Spec { cols, rows }
            }
        }
    }

    fn key_state(&self, now: Option<&KeyDraft>, was: Option<&KeyDraft>) -> RowState {
        if self.is_new() {
            return RowState::Unchanged;
        }
        match (now, was) {
            (Some(n), Some(w)) if n == w => RowState::Unchanged,
            (None, None) => RowState::Unchanged,
            (Some(_), None) => RowState::Added,
            _ => RowState::Modified,
        }
    }

    fn columns_spec(&self) -> Spec {
        let kind = self.kind;
        let mut cols = vec![
            ColSpec { field: Field::Name, title: "Name", width: 150.0 },
            ColSpec { field: Field::Type, title: "Type", width: 210.0 },
            ColSpec { field: Field::Nullable, title: "Null", width: 48.0 },
            ColSpec { field: Field::Default, title: "Default", width: 150.0 },
            ColSpec { field: Field::Pk, title: "PK", width: 44.0 },
            ColSpec { field: Field::Identity, title: "Identity", width: 104.0 },
        ];
        if ddl::supports_column_comments(kind) {
            cols.push(ColSpec { field: Field::Comment, title: "Comment", width: 0.0 });
        }
        let pk: &[u32] = self.draft.primary_key.as_ref().map_or(&[], |k| &k.columns);
        let identity_label = |i: Option<Identity>| {
            ddl::identity_choices(kind)
                .iter()
                .find(|(v, _)| *v == i)
                .map(|(_, l)| l.to_string())
                .unwrap_or_default()
        };
        let row_for = |c: &ColumnDraft, struck: bool| -> Row {
            let orig = self.original.as_ref().and_then(|o| o.columns.iter().find(|x| x.id == c.id));
            let state = match (&self.original, orig) {
                (None, _) => RowState::Unchanged,
                (Some(_), None) => RowState::Added,
                (Some(_), Some(o)) if o == c && !self.pk_changed(c.id) => RowState::Unchanged,
                _ => RowState::Modified,
            };
            let order = pk.iter().position(|i| *i == c.id);
            let generated = c.generated.as_ref();
            let mut cells = vec![
                Cell::text(Field::Name, CellKind::Text, c.name.clone()).placeholder("(name)"),
                Cell::text(Field::Type, CellKind::Type, c.data_type.clone()).placeholder("(type)"),
                Cell::toggle(Field::Nullable, c.nullable),
                match generated {
                    Some(g) => {
                        Cell::text(Field::Default, CellKind::Static, format!("AS ({})", g.expression)).dim()
                    }
                    None if c.identity.is_some() => Cell::text(Field::Default, CellKind::Static, "").dim(),
                    None => Cell::text(Field::Default, CellKind::Text, c.default.clone().unwrap_or_default())
                        .placeholder("(none)"),
                },
                {
                    let mut cell = Cell::toggle(Field::Pk, order.is_some());
                    // The position in a composite key; one column needs no number.
                    if pk.len() > 1
                        && let Some(n) = order
                    {
                        cell.text = (n + 1).to_string();
                    }
                    cell
                },
                if generated.is_some() {
                    Cell::text(Field::Identity, CellKind::Static, "generated").dim()
                } else if ddl::identity_choices(kind).len() < 2 {
                    Cell::text(Field::Identity, CellKind::Static, "").dim()
                } else {
                    Cell::text(Field::Identity, CellKind::Cycle, identity_label(c.identity))
                },
            ];
            if ddl::supports_column_comments(kind) {
                cells.push(
                    Cell::text(Field::Comment, CellKind::Text, c.comment.clone().unwrap_or_default())
                        .placeholder("(none)"),
                );
            }
            Row { id: if struck { RowId::Removed(c.id) } else { RowId::Column(c.id) }, cells, struck, state }
        };
        let mut rows: Vec<Row> = Vec::new();
        let removed: Vec<&ColumnDraft> = self
            .original
            .as_ref()
            .map(|o| o.columns.iter().filter(|c| !self.draft.columns.iter().any(|d| d.id == c.id)).collect())
            .unwrap_or_default();
        let original = self.original.as_ref();
        // A removed column is drawn where it was: after the nearest earlier column that is still here.
        let mut placed = vec![false; removed.len()];
        let place_before_first = |rows: &mut Vec<Row>, placed: &mut Vec<bool>| {
            for (n, r) in removed.iter().enumerate() {
                let pos = original.and_then(|o| o.columns.iter().position(|c| c.id == r.id)).unwrap_or(0);
                let has_earlier_live = original
                    .map(|o| o.columns[..pos].iter().any(|c| self.draft.columns.iter().any(|d| d.id == c.id)))
                    .unwrap_or(false);
                if !has_earlier_live && !placed[n] {
                    rows.push(row_for(r, true));
                    placed[n] = true;
                }
            }
        };
        place_before_first(&mut rows, &mut placed);
        for c in &self.draft.columns {
            rows.push(row_for(c, false));
            for (n, r) in removed.iter().enumerate() {
                if placed[n] {
                    continue;
                }
                let pos = original.and_then(|o| o.columns.iter().position(|x| x.id == r.id)).unwrap_or(0);
                let nearest = original.and_then(|o| {
                    o.columns[..pos]
                        .iter()
                        .rev()
                        .find(|x| self.draft.columns.iter().any(|d| d.id == x.id))
                        .map(|x| x.id)
                });
                if nearest == Some(c.id) {
                    rows.push(row_for(r, true));
                    placed[n] = true;
                }
            }
        }
        Spec { cols, rows }
    }

    /// Whether the column's primary-key membership differs from the original (the draft keeps keys apart
    /// from columns, so a column row alone cannot tell).
    fn pk_changed(&self, id: u32) -> bool {
        let Some(o) = &self.original else { return false };
        let was = o.primary_key.as_ref().is_some_and(|k| k.columns.contains(&id));
        let now = self.draft.primary_key.as_ref().is_some_and(|k| k.columns.contains(&id));
        was != now
    }
}

/// SQLite does not store constraint names, so introspection invents them (`sqlite_autoindex_t_1` for a key,
/// `fk_t_0` for a foreign key). Written back by a table rebuild they would become real constraint names
/// nobody chose: such a constraint is left unnamed instead.
fn forget_invented_names(d: &mut TableDraft) {
    let invented = |name: &str| name.starts_with("sqlite_autoindex_");
    if let Some(pk) = &mut d.primary_key
        && pk.name.as_deref().is_some_and(invented)
    {
        pk.name = None;
    }
    for u in &mut d.uniques {
        if u.name.as_deref().is_some_and(invented) {
            u.name = None;
        }
    }
    let prefix = format!("fk_{}_", d.name);
    for f in &mut d.foreign_keys {
        if let Some(rest) = f.name.as_deref().and_then(|n| n.strip_prefix(&prefix))
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
        {
            f.name = None;
        }
    }
}

fn state_of(same_as_original: Option<bool>) -> RowState {
    match same_as_original {
        None => RowState::Added,
        Some(true) => RowState::Unchanged,
        Some(false) => RowState::Modified,
    }
}

/// The key part of a `CREATE INDEX` statement, for display: everything between the first `(` and its
/// matching `)`; the whole text when it does not parse.
fn index_expression_text(definition: &str) -> String {
    let Some(start) = definition.find('(') else { return definition.to_string() };
    let mut depth = 0;
    for (i, ch) in definition[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return definition[start + 1..start + i].to_string();
                }
            }
            _ => {}
        }
    }
    definition.to_string()
}

fn bump(fresh: &mut u32) -> u32 {
    let id = *fresh;
    *fresh += 1;
    id
}

/// `orders_customer_id_idx`: what a new index is called until the user names it.
pub fn auto_index_name(table: &str, columns: &[String]) -> String {
    if table.is_empty() || columns.is_empty() {
        return String::new();
    }
    let cols = columns.iter().filter(|c| !c.is_empty()).cloned().collect::<Vec<_>>().join("_");
    if cols.is_empty() { String::new() } else { format!("{table}_{cols}_idx") }
}

/// Take a column out of the draft together with everything that cannot exist without it: it leaves keys
/// and indexes it was part of, and a constraint left with no columns goes too.
fn remove_column(d: &mut TableDraft, id: u32) {
    d.columns.retain(|c| c.id != id);
    if let Some(pk) = &mut d.primary_key {
        pk.columns.retain(|c| *c != id);
        if pk.columns.is_empty() {
            d.primary_key = None;
        }
    }
    for u in &mut d.uniques {
        u.columns.retain(|c| *c != id);
    }
    d.uniques.retain(|u| !u.columns.is_empty());
    // A foreign key with one of its columns gone is a different constraint: it goes entirely.
    d.foreign_keys.retain(|f| !f.columns.contains(&id));
    for i in &mut d.indexes {
        if i.raw.is_none() {
            i.columns.retain(|c| *c != id);
        }
    }
    d.indexes.retain(|i| i.raw.is_some() || !i.columns.is_empty());
}

fn cycle<'a>(options: &[&'a str], current: &str, step: i32) -> &'a str {
    let at = options.iter().position(|o| o.eq_ignore_ascii_case(current)).unwrap_or(0) as i32;
    options[(at + step).rem_euclid(options.len() as i32) as usize]
}

fn empty_to_none(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn set_cell(
    d: &mut TableDraft,
    fresh: &mut u32,
    kind: DatabaseKind,
    row: RowId,
    field: Field,
    value: CellValue,
) -> Result<(), String> {
    let text = |v: CellValue| match v {
        CellValue::Text(t) => Ok(t),
        _ => Err("This cell takes text".to_string()),
    };
    match (row, field) {
        (RowId::Column(id), _) => {
            let Some(c) = d.columns.iter().position(|c| c.id == id) else { return Ok(()) };
            match field {
                Field::Name => d.columns[c].name = text(value)?.trim().to_string(),
                Field::Type => d.columns[c].data_type = text(value)?.trim().to_string(),
                Field::Nullable => d.columns[c].nullable = !d.columns[c].nullable,
                Field::Default => {
                    if d.columns[c].identity.is_some() || d.columns[c].generated.is_some() {
                        return Err("A generated column has no default".into());
                    }
                    d.columns[c].default = empty_to_none(text(value)?);
                }
                Field::Comment => d.columns[c].comment = empty_to_none(text(value)?),
                Field::Pk => {
                    let in_key = d.primary_key.as_ref().is_some_and(|k| k.columns.contains(&id));
                    if in_key {
                        if let Some(k) = &mut d.primary_key {
                            k.columns.retain(|x| *x != id);
                            if k.columns.is_empty() {
                                d.primary_key = None;
                            }
                        }
                    } else {
                        let key = d.primary_key.get_or_insert_with(|| KeyDraft {
                            id: bump(fresh),
                            name: None,
                            columns: vec![],
                        });
                        key.columns.push(id);
                        // A key column cannot hold NULL; every database tool makes the same call here.
                        d.columns[c].nullable = false;
                    }
                }
                Field::Identity => {
                    if d.columns[c].generated.is_some() {
                        return Err("A generated column cannot also be an identity".into());
                    }
                    let choices = ddl::identity_choices(kind);
                    if choices.len() < 2 {
                        return Err("This database has no identity columns".into());
                    }
                    let step = match value {
                        CellValue::Step(s) => s,
                        _ => 1,
                    };
                    let at =
                        choices.iter().position(|(v, _)| *v == d.columns[c].identity).unwrap_or(0) as i32;
                    let next = choices[(at + step).rem_euclid(choices.len() as i32) as usize].0;
                    d.columns[c].identity = next;
                    if next.is_some() {
                        d.columns[c].default = None;
                        d.columns[c].nullable = false;
                    }
                }
                _ => return Err("This cell cannot be edited".into()),
            }
            Ok(())
        }
        (RowId::Removed(_), _) => Err("This column is removed; restore it first".into()),
        (RowId::Pk, Field::KeyName) => {
            let name = empty_to_none(text(value)?);
            if kind.family() == Family::MySql {
                return Err("MySQL always calls the primary key PRIMARY".into());
            }
            match (&mut d.primary_key, name) {
                (Some(k), n) => k.name = n,
                (None, Some(_)) => return Err("Choose the key's columns first".into()),
                (None, None) => {}
            }
            Ok(())
        }
        (RowId::Unique(id), Field::KeyName) => {
            let name = empty_to_none(text(value)?);
            if let Some(k) = d.uniques.iter_mut().find(|k| k.id == id) {
                k.name = name;
            }
            Ok(())
        }
        (RowId::Fk(id), _) => {
            let Some(f) = d.foreign_keys.iter_mut().find(|f| f.id == id) else { return Ok(()) };
            match field {
                Field::FkName => f.name = empty_to_none(text(value)?),
                Field::FkOnDelete | Field::FkOnUpdate => {
                    let step = match value {
                        CellValue::Step(s) => s,
                        _ => 1,
                    };
                    let slot = if field == Field::FkOnDelete { &mut f.on_delete } else { &mut f.on_update };
                    let now = slot.clone().unwrap_or_else(|| "NO ACTION".into());
                    let next = cycle(ddl::fk_actions(kind), &now, step);
                    *slot = (next != "NO ACTION").then(|| next.to_string());
                }
                Field::FkDeferrable => {
                    f.deferrable = !f.deferrable;
                    if !f.deferrable {
                        f.initially_deferred = false;
                    }
                }
                _ => return Err("This cell cannot be edited here".into()),
            }
            Ok(())
        }
        (RowId::Index(id), _) => {
            let Some(i) = d.indexes.iter_mut().find(|i| i.id == id) else { return Ok(()) };
            match field {
                Field::IdxName => i.name = text(value)?.trim().to_string(),
                Field::IdxUnique => i.unique = !i.unique,
                Field::IdxMethod => {
                    let methods = ddl::index_methods(kind);
                    if methods.is_empty() {
                        return Err("This database has one index type".into());
                    }
                    let step = match value {
                        CellValue::Step(s) => s,
                        _ => 1,
                    };
                    let now = i.method.clone().unwrap_or_else(|| methods[0].to_string());
                    let next = cycle(methods, &now, step);
                    // The default method is left implicit so an untouched index stays "unchanged".
                    i.method = (!next.eq_ignore_ascii_case(methods[0])).then(|| next.to_string());
                }
                Field::IdxPredicate => i.predicate = empty_to_none(text(value)?),
                _ => return Err("This cell cannot be edited here".into()),
            }
            Ok(())
        }
        (RowId::Check(id), _) => {
            let Some(c) = d.checks.iter_mut().find(|c| c.id == id) else { return Ok(()) };
            match field {
                Field::CheckName => c.name = empty_to_none(text(value)?),
                Field::CheckExpr => c.expression = text(value)?.trim().to_string(),
                _ => return Err("This cell cannot be edited here".into()),
            }
            Ok(())
        }
        (RowId::Comment, Field::OptValue) => {
            d.comment = empty_to_none(text(value)?);
            Ok(())
        }
        _ => Err("This cell cannot be edited".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{ColumnInfo, ForeignKey, Index, Schema};

    fn col(name: &str, ty: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: ty.into(), nullable, ..Default::default() }
    }

    fn customers() -> Table {
        Table {
            name: "customers".into(),
            columns: vec![
                col("id", "integer", false),
                col("name", "text", false),
                col("email", "text", true),
            ],
            primary_key: vec!["id".into()],
            indexes: vec![
                Index {
                    name: "customers_pkey".into(),
                    columns: vec!["id".into()],
                    unique: true,
                    primary: true,
                    ..Default::default()
                },
                Index {
                    name: "customers_name_idx".into(),
                    columns: vec!["name".into()],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn modify() -> DraftModel {
        DraftModel::modify(DatabaseKind::Postgres, "public", &customers(), None)
    }

    fn sql(m: &DraftModel) -> Vec<String> {
        m.analysis().statements.iter().map(|s| s.sql.clone()).collect()
    }

    #[test]
    fn an_untouched_table_has_no_changes_and_no_undo() {
        let m = modify();
        assert!(!m.dirty());
        assert!(m.analysis().statements.is_empty());
        assert_eq!(m.analysis().summary, "No changes");
        assert!(!m.can_undo());
    }

    #[test]
    fn a_new_table_starts_with_an_id_key_and_previews_nothing_until_it_is_touched() {
        let mut m = DraftModel::new_table(DatabaseKind::Postgres, Some("public".into()), None);
        assert!(!m.dirty());
        assert!(m.analysis().statements.is_empty());
        m.set_table_name("things");
        assert!(m.dirty());
        assert_eq!(
            sql(&m),
            vec![
                "CREATE TABLE public.things (\n    id integer GENERATED BY DEFAULT AS IDENTITY NOT NULL,\n    PRIMARY KEY (id)\n)"
            ]
        );
        assert_eq!(ddl::create_table(m.draft(), DatabaseKind::Postgres)[0].sql, sql(&m)[0]);
    }

    #[test]
    fn rename_add_and_drop_produce_the_engine_script_in_engine_order() {
        let mut m = modify();
        m.set_cell(RowId::Column(3), Field::Name, CellValue::Text("mail".into())).unwrap();
        let new = m.add_row(Section::Columns, Some(RowId::Column(3)), false).unwrap();
        m.set_cell(new, Field::Name, CellValue::Text("phone".into())).unwrap();
        m.delete_row(RowId::Column(2)).unwrap();
        assert_eq!(
            sql(&m),
            vec![
                // The index on the dropped column goes first, by name, so the preview says so.
                "DROP INDEX public.customers_name_idx",
                "ALTER TABLE public.customers DROP COLUMN name",
                "ALTER TABLE public.customers RENAME COLUMN email TO mail",
                "ALTER TABLE public.customers ADD COLUMN phone text",
            ]
        );
        assert!(m.analysis().statements[1].is_destructive());
        assert_eq!(m.analysis().summary, "4 statements · 2 destructive");
    }

    #[test]
    fn a_removed_column_stays_visible_struck_through_and_can_be_restored() {
        let mut m = modify();
        m.delete_row(RowId::Column(2)).unwrap();
        let spec = m.spec(Section::Columns);
        let ids: Vec<RowId> = spec.rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![RowId::Column(1), RowId::Removed(2), RowId::Column(3)], "drawn where it was");
        assert!(spec.rows[1].struck);
        // The same key again brings it back, and the script is empty once more.
        m.delete_row(RowId::Removed(2)).unwrap();
        assert!(!m.dirty());
        assert_eq!(m.spec(Section::Columns).rows.len(), 3);
    }

    #[test]
    fn a_removed_first_column_is_listed_first() {
        let mut m = modify();
        m.delete_row(RowId::Column(1)).unwrap();
        let ids: Vec<RowId> = m.spec(Section::Columns).rows.iter().map(|r| r.id).collect();
        assert_eq!(ids[0], RowId::Removed(1));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn deleting_a_key_column_takes_the_key_along_and_restoring_gives_it_back() {
        let mut m = modify();
        m.delete_row(RowId::Column(1)).unwrap();
        assert!(m.draft().primary_key.is_none());
        m.restore_column(1);
        assert_eq!(m.draft().primary_key.as_ref().map(|k| k.columns.clone()), Some(vec![1]));
        assert!(!m.dirty(), "restoring leaves the table exactly as it was");
    }

    #[test]
    fn deleting_an_indexed_column_drops_the_index_and_restoring_recreates_nothing_new() {
        let mut m = modify();
        m.delete_row(RowId::Column(2)).unwrap();
        assert!(m.draft().indexes.is_empty());
        assert!(sql(&m).iter().any(|s| s.starts_with("DROP INDEX")));
        m.restore_column(2);
        assert!(!m.dirty());
    }

    #[test]
    fn new_ids_never_reuse_a_removed_columns_id() {
        let mut m = modify();
        m.delete_row(RowId::Column(3)).unwrap();
        let RowId::Column(new) = m.add_row(Section::Columns, None, false).unwrap() else { panic!() };
        assert!(new > 3 && !customers().columns.is_empty());
        m.set_cell(RowId::Column(new), Field::Name, CellValue::Text("fresh".into())).unwrap();
        // If the id had been reused the diff would have read it as a change of email, not a drop plus an add.
        let out = sql(&m);
        assert!(out.iter().any(|s| s.contains("DROP COLUMN email")), "{out:?}");
        assert!(out.iter().any(|s| s.contains("ADD COLUMN fresh")), "{out:?}");
    }

    #[test]
    fn undo_and_redo_walk_the_edits_and_a_noop_leaves_no_step() {
        let mut m = modify();
        m.set_cell(RowId::Column(2), Field::Name, CellValue::Text("name".into())).unwrap();
        assert!(!m.can_undo(), "writing the same value is not an edit");
        m.set_cell(RowId::Column(2), Field::Name, CellValue::Text("title".into())).unwrap();
        m.set_cell(RowId::Column(2), Field::Type, CellValue::Text("varchar(20)".into())).unwrap();
        assert!(m.undo());
        assert_eq!(m.draft().columns[1].data_type, "text");
        assert_eq!(m.draft().columns[1].name, "title");
        assert!(m.undo());
        assert!(!m.dirty());
        assert!(!m.undo());
        assert!(m.redo() && m.redo());
        assert_eq!(m.draft().columns[1].data_type, "varchar(20)");
        assert!(!m.redo());
        // A new edit forgets the redo branch.
        m.undo();
        m.set_cell(RowId::Column(2), Field::Comment, CellValue::Text("x".into())).unwrap();
        assert!(!m.can_redo());
    }

    #[test]
    fn a_primary_key_column_becomes_not_null_and_the_key_order_is_shown() {
        let mut m = modify();
        m.set_cell(RowId::Column(3), Field::Pk, CellValue::Flip).unwrap();
        assert!(!m.draft().columns[2].nullable);
        let spec = m.spec(Section::Columns);
        let pk = |i: usize| spec.rows[i].cells.iter().find(|c| c.field == Field::Pk).unwrap().clone();
        assert!(pk(0).on && pk(0).text == "1");
        assert!(pk(2).on && pk(2).text == "2");
        assert!(!pk(1).on);
        m.set_cell(RowId::Column(3), Field::Pk, CellValue::Flip).unwrap();
        let spec = m.spec(Section::Columns);
        assert_eq!(
            spec.rows[0].cells.iter().find(|c| c.field == Field::Pk).unwrap().text,
            "",
            "one column needs no number"
        );
    }

    #[test]
    fn identity_cycles_through_the_dialects_choices_and_clears_the_default() {
        let mut m = modify();
        m.set_cell(RowId::Column(3), Field::Default, CellValue::Text("'x'".into())).unwrap();
        m.set_cell(RowId::Column(3), Field::Identity, CellValue::Step(1)).unwrap();
        assert_eq!(m.draft().columns[2].identity, Some(Identity::ByDefault));
        assert_eq!(m.draft().columns[2].default, None);
        m.set_cell(RowId::Column(3), Field::Identity, CellValue::Step(1)).unwrap();
        assert_eq!(m.draft().columns[2].identity, Some(Identity::Always));
        m.set_cell(RowId::Column(3), Field::Identity, CellValue::Step(1)).unwrap();
        assert_eq!(m.draft().columns[2].identity, None);
        let err = m.set_cell(RowId::Column(3), Field::Identity, CellValue::Step(-1));
        assert!(err.is_ok());
        let mysql = DraftModel::modify(DatabaseKind::MySql, "d", &customers(), None);
        assert_eq!(mysql.spec(Section::Columns).cols.len(), 7, "MySQL has column comments");
        let sqlite = DraftModel::modify(DatabaseKind::Sqlite, "main", &customers(), None);
        assert_eq!(sqlite.spec(Section::Columns).cols.len(), 6, "SQLite has none");
    }

    #[test]
    fn a_failed_cell_edit_changes_nothing() {
        let mut m = modify();
        m.set_cell(RowId::Column(3), Field::Identity, CellValue::Step(1)).unwrap();
        let before = m.draft().clone();
        let err = m.set_cell(RowId::Column(3), Field::Default, CellValue::Text("1".into())).unwrap_err();
        assert!(err.contains("generated"), "{err}");
        assert_eq!(m.draft(), &before);
        assert!(m.set_cell(RowId::Removed(2), Field::Name, CellValue::Text("x".into())).is_err());
    }

    #[test]
    fn only_a_new_table_can_reorder_columns() {
        let mut m = modify();
        assert!(m.move_column(2, -1).is_err());
        let mut n = DraftModel::new_table(DatabaseKind::Postgres, None, None);
        let RowId::Column(b) = n.add_row(Section::Columns, None, false).unwrap() else { panic!() };
        assert_eq!(n.move_column(b, -1), Ok(true));
        assert_eq!(n.draft().columns[0].id, b);
        assert_eq!(n.move_column(b, -1), Ok(false), "already first");
    }

    #[test]
    fn duplicating_a_column_copies_its_shape_but_not_its_key_or_identity() {
        let mut n = DraftModel::new_table(DatabaseKind::Postgres, None, None);
        let RowId::Column(c) = n.duplicate_column(1).unwrap() else { panic!() };
        let copy = n.draft().columns.iter().find(|x| x.id == c).unwrap();
        assert_eq!(copy.name, "id_copy");
        assert!(copy.identity.is_none());
        assert_eq!(n.draft().primary_key.as_ref().unwrap().columns, vec![1]);
    }

    #[test]
    fn index_names_follow_their_columns_until_the_user_names_them() {
        let mut m = modify();
        let RowId::Index(i) = m.add_row(Section::Indexes, None, false).unwrap() else { panic!() };
        assert_eq!(m.draft().indexes.last().unwrap().name, "");
        m.set_columns(RowId::Index(i), vec![3]);
        assert_eq!(m.draft().indexes.last().unwrap().name, "customers_email_idx");
        m.set_columns(RowId::Index(i), vec![3, 2]);
        assert_eq!(m.draft().indexes.last().unwrap().name, "customers_email_name_idx");
        m.set_cell(RowId::Index(i), Field::IdxName, CellValue::Text("mine".into())).unwrap();
        m.set_columns(RowId::Index(i), vec![2]);
        assert_eq!(m.draft().indexes.last().unwrap().name, "mine");
        assert_eq!(sql(&m), vec!["CREATE INDEX mine ON public.customers (name)"]);
    }

    #[test]
    fn adding_an_index_is_one_undo_step() {
        let mut m = modify();
        m.add_row(Section::Indexes, None, false).unwrap();
        assert!(m.undo());
        assert!(!m.dirty());
        assert!(!m.can_undo());
    }

    #[test]
    fn a_foreign_key_defaults_its_referenced_columns_to_the_targets_primary_key() {
        let parent = Table {
            name: "regions".into(),
            columns: vec![col("code", "integer", false)],
            primary_key: vec!["code".into()],
            ..Default::default()
        };
        let catalog = Arc::new(Catalog {
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![customers(), parent],
                ..Default::default()
            }],
            default_schema: Some("public".into()),
        });
        let mut m = DraftModel::modify(DatabaseKind::Postgres, "public", &customers(), Some(catalog));
        let RowId::Fk(f) = m.add_row(Section::ForeignKeys, None, false).unwrap() else { panic!() };
        // Incomplete foreign keys are errors, and the section says so.
        assert_eq!(m.analysis().worst(Section::ForeignKeys), Some(Severity::Error));
        m.set_columns(RowId::Fk(f), vec![3]);
        m.set_fk_target(f, Some("public".into()), "regions");
        assert_eq!(m.draft().foreign_keys[0].ref_columns, vec!["code".to_string()]);
        m.set_cell(RowId::Fk(f), Field::FkOnDelete, CellValue::Step(1)).unwrap();
        m.set_cell(RowId::Fk(f), Field::FkOnDelete, CellValue::Step(1)).unwrap();
        assert_eq!(m.draft().foreign_keys[0].on_delete.as_deref(), Some("CASCADE"));
        // A type mismatch is a warning from the engine (text email against integer code).
        assert_eq!(m.analysis().worst(Section::ForeignKeys), Some(Severity::Warning));
        let out = sql(&m);
        assert_eq!(
            out,
            vec![
                "ALTER TABLE public.customers ADD FOREIGN KEY (email) REFERENCES public.regions (code) ON DELETE CASCADE"
            ]
        );
    }

    #[test]
    fn a_name_that_is_taken_is_an_error_in_the_table_section() {
        let other = Table { name: "orders".into(), ..Default::default() };
        let catalog = Arc::new(Catalog {
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![customers(), other],
                ..Default::default()
            }],
            default_schema: Some("public".into()),
        });
        let mut m = DraftModel::modify(DatabaseKind::Postgres, "public", &customers(), Some(catalog));
        m.set_table_name("orders");
        assert!(m.analysis().issues.iter().any(|i| i.message.contains("already exists")));
        assert!(m.analysis().errors() > 0);
    }

    #[test]
    fn fixes_resolve_warnings_and_validation_marks_sections() {
        let mut m = modify();
        m.set_columns(RowId::Pk, vec![3]);
        assert!(!m.draft().columns[2].nullable, "setting key columns makes them NOT NULL");
        let mut n = DraftModel::new_table(DatabaseKind::Postgres, None, None);
        n.set_cell(RowId::Column(1), Field::Nullable, CellValue::Flip).unwrap();
        let warning = n.analysis().issues.iter().find(|i| i.fix.is_some()).cloned().unwrap();
        assert_eq!(n.analysis().worst(Section::Keys), Some(Severity::Warning));
        n.apply_fix(warning.fix.as_ref().unwrap());
        assert!(!n.draft().columns[0].nullable);
        assert_eq!(n.analysis().worst(Section::Keys), None);
    }

    #[test]
    fn sqlite_does_not_write_its_invented_constraint_names_back() {
        let mut t = customers();
        t.indexes = vec![Index {
            name: "sqlite_autoindex_customers_1".into(),
            columns: vec!["id".into()],
            unique: true,
            primary: true,
            ..Default::default()
        }];
        t.foreign_keys = vec![ForeignKey {
            name: "fk_customers_0".into(),
            columns: vec!["email".into()],
            referenced_table: "t".into(),
            referenced_columns: vec!["e".into()],
            ..Default::default()
        }];
        let m = DraftModel::modify(DatabaseKind::Sqlite, "main", &t, None);
        assert_eq!(m.draft().primary_key.as_ref().unwrap().name, None);
        assert_eq!(m.draft().foreign_keys[0].name, None);
        assert_eq!(m.draft().schema, None);
        // Other databases keep what the catalog says, even a name that looks like that.
        let pg = DraftModel::modify(DatabaseKind::Postgres, "public", &t, None);
        assert_eq!(pg.draft().foreign_keys[0].name.as_deref(), Some("fk_customers_0"));
        // A name the user chose survives in SQLite too.
        t.foreign_keys[0].name = "fk_to_t".into();
        let kept = DraftModel::modify(DatabaseKind::Sqlite, "main", &t, None);
        assert_eq!(kept.draft().foreign_keys[0].name.as_deref(), Some("fk_to_t"));
    }

    #[test]
    fn triggers_are_replayed_after_a_sqlite_rebuild() {
        let mut m = DraftModel::modify(DatabaseKind::Sqlite, "main", &customers(), None);
        m.set_recreate(vec!["CREATE TRIGGER trg AFTER INSERT ON customers BEGIN SELECT 1; END".into()]);
        assert!(!m.dirty(), "handing over the triggers is not an edit");
        m.set_cell(RowId::Column(2), Field::Type, CellValue::Text("VARCHAR(20)".into())).unwrap();
        let out = sql(&m);
        assert!(out.iter().any(|s| s.starts_with("CREATE TRIGGER trg")), "{out:?}");
        let at = |needle: &str| out.iter().position(|s| s.starts_with(needle)).unwrap();
        assert!(at("ALTER TABLE customers__new RENAME") < at("CREATE TRIGGER"));
    }

    #[test]
    fn accepting_makes_the_applied_edit_the_new_original() {
        let mut n = DraftModel::new_table(DatabaseKind::Postgres, Some("public".into()), None);
        n.set_table_name("things");
        assert!(n.is_new() && n.dirty());
        n.accept();
        assert!(!n.is_new() && !n.dirty());
        assert!(n.analysis().statements.is_empty());
        assert!(!n.can_undo());
        // Further edits are alterations of the table that now exists.
        n.set_cell(RowId::Column(1), Field::Name, CellValue::Text("key".into())).unwrap();
        assert_eq!(sql(&n), vec!["ALTER TABLE public.things RENAME COLUMN id TO key"]);
    }

    #[test]
    fn revert_restores_the_table_and_can_be_undone() {
        let mut m = modify();
        m.delete_row(RowId::Column(2)).unwrap();
        m.set_table_name("clients");
        m.revert();
        assert!(!m.dirty());
        assert!(m.undo());
        assert_eq!(m.draft().name, "clients");
    }

    #[test]
    fn sections_report_their_rows() {
        let mut t = customers();
        t.foreign_keys = vec![ForeignKey {
            name: "customers_x_fk".into(),
            columns: vec!["email".into()],
            referenced_table: "t".into(),
            referenced_columns: vec!["e".into()],
            ..Default::default()
        }];
        let mut m = DraftModel::modify(DatabaseKind::Postgres, "public", &t, None);
        assert_eq!(m.count(Section::Columns), 3);
        assert_eq!(m.count(Section::Keys), 1);
        assert_eq!(m.count(Section::ForeignKeys), 1);
        assert_eq!(m.count(Section::Indexes), 1, "the primary key's index is part of the key");
        let fk = m.spec(Section::ForeignKeys);
        assert_eq!(fk.rows[0].cells[1].chips, vec!["email".to_string()]);
        assert_eq!(fk.cols.len(), 7);
        assert_eq!(
            DraftModel::modify(DatabaseKind::MySql, "d", &t, None).spec(Section::ForeignKeys).cols.len(),
            6
        );
        // Editing marks the row, not its neighbours.
        m.set_cell(RowId::Fk(m.draft().foreign_keys[0].id), Field::FkDeferrable, CellValue::Flip).unwrap();
        assert_eq!(m.spec(Section::ForeignKeys).rows[0].state, RowState::Modified);
        assert_eq!(m.spec(Section::Columns).rows[0].state, RowState::Unchanged);
        let cols = m.spec(Section::Columns);
        assert_eq!(cols.rows.len(), 3);
    }

    #[test]
    fn expression_indexes_show_their_definition_and_are_not_editable_as_columns() {
        let mut t = customers();
        t.indexes.push(Index {
            name: "customers_lower_idx".into(),
            columns: vec![],
            has_expressions: true,
            definition: Some(
                "CREATE INDEX customers_lower_idx ON public.customers USING btree (lower(email))".into(),
            ),
            ..Default::default()
        });
        let m = DraftModel::modify(DatabaseKind::Postgres, "public", &t, None);
        let spec = m.spec(Section::Indexes);
        let row =
            spec.rows.iter().find(|r| r.id == RowId::Index(m.draft().indexes.last().unwrap().id)).unwrap();
        assert_eq!(row.cells[1].kind, CellKind::Static);
        assert_eq!(row.cells[1].chips, vec!["lower(email)".to_string()]);
        assert!(!m.dirty());
    }

    #[test]
    fn options_show_the_comment_and_the_read_only_storage_settings() {
        let mut t = customers();
        t.options = vec![theta_core::TableOption { name: "ENGINE".into(), value: "InnoDB".into() }];
        let m = DraftModel::modify(DatabaseKind::MySql, "d", &t, None);
        let spec = m.spec(Section::Options);
        assert_eq!(spec.rows.len(), 2);
        assert_eq!(spec.rows[1].cells[1].kind, CellKind::Static);
        let sqlite = DraftModel::modify(DatabaseKind::Sqlite, "main", &customers(), None);
        assert!(sqlite.spec(Section::Options).rows.is_empty());
        let mut pg = modify();
        pg.set_cell(RowId::Comment, Field::OptValue, CellValue::Text("People".into())).unwrap();
        assert_eq!(sql(&pg), vec!["COMMENT ON TABLE public.customers IS 'People'"]);
    }
}
