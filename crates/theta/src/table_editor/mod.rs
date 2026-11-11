//! The Modify Table / New Table editor: a workspace tab that edits a [`DraftModel`] and shows the script it
//! amounts to.
//!
//! Layers, from the bottom:
//!
//! * `theta_sql::ddl` decides what a change costs (the statements, their flags, the problems);
//! * [`model`] decides what an edit means and keeps undo, as plain data (tested without a window);
//! * [`keys`] maps a key press to a command (also without a window);
//! * this module owns the tab's state (cursor, the cell being edited, popups, the run state) and applies
//!   commands to the model; [`render`] paints it.
//!
//! The editor never talks to a database. It emits [`TableEditorEvent`]s (Execute, pick a table, open the
//! script in a console) and the workspace answers them with its transaction mode, its confirmation
//! dialogs and its catalog refresh, exactly as for the explorer's table actions.

pub mod highlight;
pub mod keys;
pub mod model;
mod render;

use crate::table_editor::keys::{Cmd, Press};
use crate::table_editor::model::{CellKind, CellValue, DraftModel, Field, RowId, Section, Spec};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Subscription, UniformListScrollHandle, Window, actions,
};
use std::collections::HashMap;
use std::sync::Arc;
use theta_core::{Catalog, DatabaseKind, Table};
use theta_sql::VirtualForeignKey;
use theta_sql::ddl::{self, Fix, Statement};
use theta_ui::Settings;

actions!(theta_table_editor, [TabNextCell, TabPrevCell, EditNext, EditPrev, EditCancel]);

/// The editor's own key bindings. Tab is bound to focus traversal by gpui-kit's `Root`, so the cell cursor
/// has to claim it for its context (the same reason `theta-grid` does).
pub fn bind_keys(cx: &mut gpui_kit::App) {
    use gpui_kit::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("tab", TabNextCell, Some("TableEditor")),
        KeyBinding::new("shift-tab", TabPrevCell, Some("TableEditor")),
        KeyBinding::new("tab", EditNext, Some("TableEditorCell > Input")),
        KeyBinding::new("shift-tab", EditPrev, Some("TableEditorCell > Input")),
        KeyBinding::new("escape", EditCancel, Some("TableEditorCell > Input")),
        KeyBinding::new("escape", EditCancel, Some("TableEditorName > Input")),
    ]);
}

/// What the editor asks of the workspace.
#[derive(Debug, Clone, PartialEq)]
pub enum TableEditorEvent {
    /// Run the script (the editor has checked that it is runnable).
    Execute,
    /// Choose the table a foreign key points at.
    PickTable { fk: u32 },
    /// Choose one of the suggested foreign keys.
    PickSuggestion,
    /// Open the script in a console.
    OpenInConsole(String),
    /// The tab's title or its unapplied-changes mark changed.
    StateChanged,
    /// The table was reloaded from the database after a successful Execute: reveal it in the explorer.
    Reloaded { schema: String, table: String },
}

impl EventEmitter<TableEditorEvent> for TableEditor {}

/// Where the run of the script stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    Idle,
    Running,
    /// The script stopped at `statement` (an index into the previewed statements) with the server's `message`.
    Failed {
        statement: Option<usize>,
        message: String,
        note: String,
    },
}

/// The cell being edited.
pub(crate) struct CellEdit {
    pub input: Entity<InputState>,
    pub row: RowId,
    pub field: Field,
    pub kind: CellKind,
    /// Type suggestions for the text typed so far, and the one the arrow keys are on.
    pub suggestions: Vec<&'static str>,
    pub highlighted: Option<usize>,
    pub error: Option<String>,
    pub _subs: Vec<Subscription>,
}

/// A small list for choosing and ordering columns.
pub(crate) struct ListPopup {
    pub row: RowId,
    pub field: Field,
    /// `(key, label)` of everything that can be chosen: a column id, or a referenced column's name.
    pub items: Vec<(String, String)>,
    /// Keys of what is chosen, in order.
    pub chosen: Vec<String>,
    pub cursor: usize,
}

/// What the tab shows as its title: "Modify customers" / "New table".
pub fn title_for(model: &DraftModel) -> String {
    match model.original() {
        Some(o) => format!("Modify {}", o.name),
        None => match model.draft().name.trim() {
            "" => "New table".to_string(),
            n => format!("New table {n}"),
        },
    }
}

pub struct TableEditor {
    pub(crate) model: DraftModel,
    pub(crate) conn_id: String,
    pub(crate) conn_name: String,
    pub(crate) read_only: bool,
    pub(crate) section: Section,
    /// Row and column of the cell cursor, per section.
    pub(crate) cursors: HashMap<Section, (usize, usize)>,
    pub(crate) focus: FocusHandle,
    pub(crate) name_input: Entity<InputState>,
    pub(crate) editing: Option<CellEdit>,
    pub(crate) popup: Option<ListPopup>,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) preview_open: bool,
    pub(crate) preview_width: f32,
    /// One line under the grid: the last thing that went wrong, or what a key did.
    pub(crate) status: Option<(bool, String)>,
    pub(crate) pending: keys::Pending,
    pub(crate) run: Run,
    /// Foreign keys the catalog suggests for this table (from `infer_foreign_keys`).
    pub(crate) suggestions: Vec<VirtualForeignKey>,
    /// After a successful Execute: the table to reload once the refreshed catalog has it.
    pub(crate) rebase: Option<(Option<String>, String)>,
    pub(crate) _subs: Vec<Subscription>,
}

impl Focusable for TableEditor {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TableEditor {
    pub fn new(
        model: DraftModel,
        conn_id: String,
        conn_name: String,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = model.draft().name.clone();
        let name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("table_name").default_value(name));
        let subs =
            vec![cx.subscribe_in(&name_input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                match ev {
                    InputEvent::PressEnter { .. } => {
                        this.commit_name(cx);
                        window.focus(&this.focus, cx);
                    }
                    InputEvent::Blur => this.commit_name(cx),
                    _ => {}
                }
            })];
        let mut editor = Self {
            model,
            conn_id,
            conn_name,
            read_only,
            section: Section::Columns,
            cursors: HashMap::new(),
            focus: cx.focus_handle(),
            name_input,
            editing: None,
            popup: None,
            scroll: UniformListScrollHandle::new(),
            preview_open: true,
            preview_width: 440.0,
            status: None,
            pending: None,
            run: Run::Idle,
            suggestions: Vec::new(),
            rebase: None,
            _subs: subs,
        };
        editor.refresh_suggestions();
        editor
    }

    // ------------------------------------------------------------ reading

    pub fn title(&self) -> String {
        title_for(&self.model)
    }

    /// Unapplied edits (the tab shows a dot and asks before closing).
    pub fn dirty(&self) -> bool {
        self.model.dirty()
    }

    pub fn model(&self) -> &DraftModel {
        &self.model
    }

    pub fn conn_id(&self) -> &str {
        &self.conn_id
    }

    pub fn kind(&self) -> DatabaseKind {
        self.model.kind()
    }

    pub fn section(&self) -> Section {
        self.section
    }

    pub fn run_state(&self) -> &Run {
        &self.run
    }

    pub fn is_running(&self) -> bool {
        self.run == Run::Running
    }

    pub fn statements(&self) -> &[Statement] {
        &self.model.analysis().statements
    }

    /// The preview as text: every statement, `;`-terminated, separated by a blank line.
    pub fn script(&self) -> String {
        self.statements().iter().map(|s| format!("{};", s.sql)).collect::<Vec<_>>().join("\n\n")
    }

    pub fn focus_handle_ref(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some() || self.popup.is_some()
    }

    pub fn cursor(&self) -> (usize, usize) {
        let spec = self.model.spec(self.section);
        self.clamped_cursor(&spec)
    }

    fn clamped_cursor(&self, spec: &Spec) -> (usize, usize) {
        let (r, c) = self.cursors.get(&self.section).copied().unwrap_or((0, 0));
        (r.min(spec.rows.len().saturating_sub(1)), c.min(spec.cols.len().saturating_sub(1)))
    }

    /// The row the cursor is on.
    pub fn current_row(&self) -> Option<RowId> {
        let spec = self.model.spec(self.section);
        spec.rows.get(self.clamped_cursor(&spec).0).map(|r| r.id)
    }

    // ------------------------------------------------------- feeding it

    /// The catalog changed (a refresh, possibly after our own Execute).
    pub fn set_catalog(
        &mut self,
        catalog: Option<Arc<Catalog>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((schema, table)) = self.rebase.clone()
            && let Some(c) = &catalog
            && let Some((s, t)) = c.find_table(schema.as_deref(), &table)
        {
            self.rebase = None;
            self.reload(s.name.clone().as_str(), t, catalog.clone(), window, cx);
            return;
        }
        self.model.set_catalog(catalog);
        self.refresh_suggestions();
        cx.notify();
    }

    /// Start over from `table` as the database now has it: a clean diff, the cursor where it was.
    fn reload(
        &mut self,
        schema: &str,
        table: &Table,
        catalog: Option<Arc<Catalog>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model = DraftModel::modify(self.model.kind(), schema, table, catalog);
        self.name_input.update(cx, |s, cx| s.set_value(table.name.clone(), window, cx));
        self.editing = None;
        self.popup = None;
        self.run = Run::Idle;
        self.refresh_suggestions();
        self.status = Some((false, "Applied. The table is reloaded from the database.".into()));
        cx.emit(TableEditorEvent::Reloaded { schema: schema.to_string(), table: table.name.clone() });
        cx.emit(TableEditorEvent::StateChanged);
        cx.notify();
    }

    /// The table to reload once the next catalog has it (after a successful Execute).
    pub fn expect_table(&mut self, schema: Option<String>, table: String) {
        self.rebase = Some((schema, table));
    }

    /// The script ran. Normally the tab then reloads from the refreshed catalog (see [`Self::expect_table`]);
    /// when the change sits in the user's open transaction the database cannot say what it looks like, so
    /// the edit itself becomes the new original.
    pub fn applied(&mut self, in_session: bool, cx: &mut Context<Self>) {
        self.run = Run::Idle;
        let (schema, name) = (self.model.draft().schema.clone(), self.model.draft().name.clone());
        if in_session {
            self.model.accept();
            self.status = Some((
                false,
                "Applied inside your open transaction; nothing is final until you commit.".into(),
            ));
        } else {
            self.expect_table(schema, name);
            self.status = Some((false, "Applied. Reloading the table from the database…".into()));
        }
        cx.emit(TableEditorEvent::StateChanged);
        cx.notify();
    }

    /// A foreign key's target table was chosen in the picker.
    pub fn choose_fk_target(&mut self, fk: u32, schema: Option<String>, table: &str, cx: &mut Context<Self>) {
        let schema = schema.filter(|_| self.model.kind() != DatabaseKind::Sqlite);
        self.model.set_fk_target(fk, schema, table);
        self.after_edit(cx);
    }

    fn refresh_suggestions(&mut self) {
        self.suggestions.clear();
        let (Some(catalog), Some(orig)) = (self.model.catalog(), self.model.original()) else { return };
        let schema = orig.schema.clone().unwrap_or_default();
        let name = orig.name.clone();
        self.suggestions = theta_sql::infer_foreign_keys(catalog)
            .into_iter()
            .filter(|v| {
                v.table.eq_ignore_ascii_case(&name)
                    && (schema.is_empty() || v.schema.eq_ignore_ascii_case(&schema))
            })
            .collect();
    }

    /// The suggestions not yet taken by a foreign key in the draft.
    pub fn open_suggestions(&self) -> Vec<VirtualForeignKey> {
        let d = self.model.draft();
        self.suggestions
            .iter()
            .filter(|v| {
                !d.foreign_keys.iter().any(|f| {
                    f.columns.len() == 1
                        && d.columns
                            .iter()
                            .any(|c| c.id == f.columns[0] && c.name.eq_ignore_ascii_case(&v.column))
                })
            })
            .cloned()
            .collect()
    }

    /// Add `v` as a foreign key of this table, switching to the Foreign Keys section.
    pub fn add_suggested(&mut self, v: &VirtualForeignKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(col) =
            self.model.draft().columns.iter().find(|c| c.name.eq_ignore_ascii_case(&v.column)).map(|c| c.id)
        else {
            self.say(true, format!("This table has no column {}", v.column), cx);
            return;
        };
        let schema = (self.model.kind() != DatabaseKind::Sqlite).then(|| v.referenced_schema.clone());
        let Some(RowId::Fk(id)) = self.model.add_row(Section::ForeignKeys, None, false) else { return };
        self.model.set_columns(RowId::Fk(id), vec![col]);
        self.model.set_fk_target(id, schema, &v.referenced_table);
        self.model.set_ref_columns(id, vec![v.referenced_column.clone()]);
        self.switch_section(Section::ForeignKeys, window, cx);
        let spec = self.model.spec(Section::ForeignKeys);
        if let Some(r) = spec.index_of(RowId::Fk(id)) {
            self.cursors.insert(Section::ForeignKeys, (r, 0));
        }
        self.after_edit(cx);
    }

    /// Put the cursor on a column (for "Modify Column…").
    pub fn focus_column(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_section(Section::Columns, window, cx);
        let spec = self.model.spec(Section::Columns);
        if let Some(r) = spec.rows.iter().position(|r| {
            matches!(r.id, RowId::Column(_))
                && r.cells.first().is_some_and(|c| c.text.eq_ignore_ascii_case(name))
        }) {
            self.cursors.insert(Section::Columns, (r, 0));
            self.scroll.scroll_to_item(r, gpui_kit::ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Set the table's name (the field and the model together).
    pub fn set_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.name_input.update(cx, |s, cx| s.set_value(name.to_string(), window, cx));
        self.commit_name(cx);
    }

    pub fn focus_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.name_input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.name_input.update(cx, |s, cx| s.select_all(window, cx));
    }

    // ------------------------------------------------------------ changes

    pub(crate) fn say(&mut self, error: bool, text: impl Into<String>, cx: &mut Context<Self>) {
        self.status = Some((error, text.into()));
        cx.notify();
    }

    /// Everything that follows a change to the model: the title and the dirty mark, a repaint.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        // An edit makes an earlier failure stale: the preview is a new script now.
        if matches!(self.run, Run::Failed { .. }) {
            self.run = Run::Idle;
        }
        cx.emit(TableEditorEvent::StateChanged);
        cx.notify();
    }

    fn commit_name(&mut self, cx: &mut Context<Self>) {
        let text = self.name_input.read(cx).value().to_string();
        if text.trim() != self.model.draft().name {
            self.model.set_table_name(&text);
            self.after_edit(cx);
        }
    }

    /// Set a cell and report the engine's refusal, if any.
    pub fn set_cell(&mut self, row: RowId, field: Field, value: CellValue, cx: &mut Context<Self>) -> bool {
        match self.model.set_cell(row, field, value) {
            Ok(()) => {
                self.status = None;
                self.after_edit(cx);
                true
            }
            Err(e) => {
                self.say(true, e, cx);
                false
            }
        }
    }

    /// Add a row below the cursor (or above) and start editing it where that makes sense.
    pub fn add_row(&mut self, above: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only_notice(cx) {
            return;
        }
        let anchor = self.current_row();
        let Some(id) = self.model.add_row(self.section, anchor, above) else {
            self.say(false, "There is nothing to add in this section", cx);
            return;
        };
        let spec = self.model.spec(self.section);
        if let Some(r) = spec.index_of(id) {
            self.cursors.insert(self.section, (r, 0));
            self.scroll.scroll_to_item(r, gpui_kit::ScrollStrategy::Nearest);
        }
        self.after_edit(cx);
        // A new column wants its name; a new index its columns; a new check its expression.
        let spec = self.model.spec(self.section);
        let col = match self.section {
            Section::Checks => 1,
            _ => 0,
        };
        if let Some(r) = spec.index_of(id) {
            self.cursors.insert(self.section, (r, col));
        }
        self.activate(window, cx);
    }

    /// "New Index…" / "New Foreign Key…" from the explorer: a fresh row in `section`, its columns chosen first.
    pub fn begin_new_row(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_section(section, window, cx);
        if self.read_only_notice(cx) {
            return;
        }
        let Some(id) = self.model.add_row(section, None, false) else { return };
        let spec = self.model.spec(section);
        let row = spec.index_of(id).unwrap_or(0);
        let col = spec
            .cols
            .iter()
            .position(|c| matches!(c.field, Field::IdxColumns | Field::FkColumns))
            .unwrap_or(0);
        self.cursors.insert(section, (row, col));
        self.scroll.scroll_to_item(row, gpui_kit::ScrollStrategy::Nearest);
        self.after_edit(cx);
        self.open_list(
            id,
            if section == Section::Indexes { Field::IdxColumns } else { Field::FkColumns },
            window,
            cx,
        );
        window.focus(&self.focus, cx);
    }

    pub fn delete_current(&mut self, cx: &mut Context<Self>) {
        if self.read_only_notice(cx) {
            return;
        }
        let Some(row) = self.current_row() else { return };
        match self.model.delete_row(row) {
            Ok(()) => {
                if matches!(row, RowId::Removed(_)) {
                    self.say(false, "Column restored", cx);
                } else if matches!(row, RowId::Column(id) if self.model.original().is_some_and(|o| o.columns.iter().any(|c| c.id == id)))
                {
                    self.say(
                        false,
                        "Column will be dropped when you Execute; delete it again (or undo) to keep it",
                        cx,
                    );
                }
                self.after_edit(cx);
            }
            Err(e) => self.say(true, e, cx),
        }
    }

    pub fn duplicate_current(&mut self, cx: &mut Context<Self>) {
        if self.read_only_notice(cx) || self.section != Section::Columns {
            return;
        }
        if let Some(RowId::Column(id)) = self.current_row()
            && let Some(new) = self.model.duplicate_column(id)
        {
            let spec = self.model.spec(Section::Columns);
            if let Some(r) = spec.index_of(new) {
                let c = self.cursors.get(&Section::Columns).map_or(0, |c| c.1);
                self.cursors.insert(Section::Columns, (r, c));
            }
            self.after_edit(cx);
        }
    }

    pub fn move_current(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.read_only_notice(cx) || self.section != Section::Columns {
            return;
        }
        let Some(RowId::Column(id)) = self.current_row() else { return };
        match self.model.move_column(id, delta) {
            Ok(true) => {
                let spec = self.model.spec(Section::Columns);
                if let Some(r) = spec.index_of(RowId::Column(id)) {
                    let c = self.cursors.get(&Section::Columns).map_or(0, |c| c.1);
                    self.cursors.insert(Section::Columns, (r, c));
                }
                self.after_edit(cx);
            }
            Ok(false) => {}
            Err(e) => self.say(true, e, cx),
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if self.model.undo() {
            self.after_edit(cx);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if self.model.redo() {
            self.after_edit(cx);
        }
    }

    pub fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.revert();
        let name = self.model.draft().name.clone();
        self.name_input.update(cx, |s, cx| s.set_value(name, window, cx));
        self.after_edit(cx);
    }

    pub fn apply_fix(&mut self, fix: &Fix, cx: &mut Context<Self>) {
        self.model.apply_fix(fix);
        self.after_edit(cx);
    }

    /// Refuse (with a sentence) when the connection is read-only; `true` means "stop".
    fn read_only_notice(&mut self, cx: &mut Context<Self>) -> bool {
        if self.read_only {
            self.say(true, "This connection is read-only: the table cannot be changed", cx);
        }
        self.read_only
    }

    pub fn switch_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if self.section != section {
            self.cancel_edit(window, cx);
            self.popup = None;
            self.section = section;
            self.pending = None;
        }
        cx.notify();
    }

    fn step_section(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let all = Section::ALL;
        let at = all.iter().position(|s| *s == self.section).unwrap_or(0) as isize;
        let next = all[(at + delta).rem_euclid(all.len() as isize) as usize];
        self.switch_section(next, window, cx);
    }

    // -------------------------------------------------------------- script

    pub fn copy_script(&mut self, cx: &mut Context<Self>) {
        if self.statements().is_empty() {
            self.say(false, "There is no script yet: nothing has changed", cx);
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.script()));
        self.say(false, "Script copied", cx);
    }

    pub fn copy_statement(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(s) = self.statements().get(index) {
            let text = format!("{};", s.sql);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.say(false, "Statement copied", cx);
        }
    }

    pub fn open_in_console(&mut self, cx: &mut Context<Self>) {
        if self.statements().is_empty() {
            self.say(false, "There is no script yet: nothing has changed", cx);
            return;
        }
        cx.emit(TableEditorEvent::OpenInConsole(self.script()));
    }

    /// Why Execute cannot run right now, if it cannot.
    pub fn execute_problem(&self) -> Option<String> {
        if self.read_only {
            return Some("This connection is read-only: the table cannot be changed".into());
        }
        if self.is_running() {
            return Some("The script is already running".into());
        }
        let a = self.model.analysis();
        if a.statements.is_empty() {
            return Some("No changes to apply".into());
        }
        let errors = a.errors();
        if errors > 0 {
            return Some(format!(
                "Fix the {errors} problem{} listed above the preview first",
                if errors == 1 { "" } else { "s" }
            ));
        }
        None
    }

    pub fn request_execute(&mut self, cx: &mut Context<Self>) {
        // The name field may still hold text that was not committed yet.
        self.commit_name(cx);
        if let Some(p) = self.execute_problem() {
            self.say(true, p, cx);
            return;
        }
        cx.emit(TableEditorEvent::Execute);
    }

    pub fn set_running(&mut self, cx: &mut Context<Self>) {
        self.run = Run::Running;
        self.status = None;
        cx.notify();
    }

    pub fn set_failed(
        &mut self,
        statement: Option<usize>,
        message: String,
        note: String,
        cx: &mut Context<Self>,
    ) {
        self.run = Run::Failed { statement, message, note };
        cx.notify();
    }

    pub fn set_idle(&mut self, cx: &mut Context<Self>) {
        self.run = Run::Idle;
        cx.notify();
    }

    // ------------------------------------------------------------ editing

    /// Act on the cell under the cursor: edit text, flip a box, step a cycle, open a list or a picker.
    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spec = self.model.spec(self.section);
        let (r, c) = self.clamped_cursor(&spec);
        let (Some(row), Some(col)) = (spec.rows.get(r), spec.cols.get(c)) else { return };
        let Some(cell) = row.cells.iter().find(|x| x.field == col.field) else { return };
        let (id, field, kind) = (row.id, cell.field, cell.kind);
        if kind != CellKind::Static && self.read_only_notice(cx) {
            return;
        }
        match kind {
            CellKind::Text | CellKind::Type => {
                let text = if cell.muted { String::new() } else { cell.text.clone() };
                self.start_edit(id, field, kind, text, window, cx);
            }
            CellKind::Toggle => {
                self.set_cell(id, field, CellValue::Flip, cx);
            }
            CellKind::Cycle => {
                self.set_cell(id, field, CellValue::Step(1), cx);
            }
            CellKind::Columns => self.open_list(id, field, window, cx),
            CellKind::RefTable => {
                if let RowId::Fk(fk) = id {
                    cx.emit(TableEditorEvent::PickTable { fk });
                }
            }
            CellKind::Static => {
                if matches!(id, RowId::Removed(_)) {
                    self.say(false, "This column will be dropped; press Delete to restore it", cx);
                }
            }
        }
    }

    fn start_edit(
        &mut self,
        row: RowId,
        field: Field,
        kind: CellKind,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(row, RowId::Removed(_)) {
            self.say(false, "This column will be dropped; press Delete to restore it", cx);
            return;
        }
        self.commit_name(cx);
        let len = text.len();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subs = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.commit_edit(Next::Stay, window, cx),
                InputEvent::Blur => {
                    // A click elsewhere commits; an invalid value is dropped rather than kept half-typed.
                    if this.editing.is_some() && !this.commit_edit_inner(window, cx) {
                        this.cancel_edit(window, cx);
                    }
                }
                InputEvent::Change => this.refilter(cx),
                InputEvent::Focus => {}
            },
        )];
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(len..len, cx);
            s.select_all(window, cx);
        });
        self.editing = Some(CellEdit {
            input,
            row,
            field,
            kind,
            suggestions: Vec::new(),
            highlighted: None,
            error: None,
            _subs: subs,
        });
        self.refilter(cx);
        cx.notify();
    }

    /// Recompute the type suggestions for what is in the input.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let kind = self.model.kind();
        if let Some(e) = &mut self.editing {
            e.error = None;
            if e.kind == CellKind::Type {
                let typed = e.input.read(cx).value().to_string();
                e.suggestions = ddl::matching_types(kind, &typed).into_iter().take(8).collect();
                e.highlighted = None;
            }
        }
        cx.notify();
    }

    pub(crate) fn suggest_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.editing {
            let n = e.suggestions.len() as i32;
            if n > 0 {
                let at = e.highlighted.map_or(if delta > 0 { -1 } else { n }, |h| h as i32);
                e.highlighted = Some((at + delta).rem_euclid(n) as usize);
                cx.notify();
            }
        }
    }

    pub(crate) fn commit_edit(&mut self, then: Next, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_edit_inner(window, cx) {
            window.focus(&self.focus, cx);
            match then {
                Next::Stay => {}
                Next::Forward => self.move_next(1, window, cx, true),
                Next::Back => self.move_next(-1, window, cx, true),
            }
        }
    }

    /// `true` when the edit was accepted (or there was none).
    fn commit_edit_inner(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(e) = self.editing.take() else { return true };
        let mut text = e.input.read(cx).value().to_string();
        if let Some(h) = e.highlighted
            && let Some(s) = e.suggestions.get(h)
        {
            text = (*s).to_string();
        }
        match self.model.set_cell(e.row, e.field, CellValue::Text(text)) {
            Ok(()) => {
                self.status = None;
                self.after_edit(cx);
                true
            }
            Err(msg) => {
                self.say(true, msg, cx);
                false
            }
        }
    }

    pub(crate) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn open_list(&mut self, row: RowId, field: Field, _window: &mut Window, cx: &mut Context<Self>) {
        let (items, chosen): (Vec<(String, String)>, Vec<String>) = match field {
            Field::FkRefColumns => {
                let RowId::Fk(id) = row else { return };
                let Some(f) = self.model.draft().foreign_keys.iter().find(|f| f.id == id) else { return };
                if f.ref_table.is_empty() {
                    self.say(true, "Choose the referenced table first", cx);
                    return;
                }
                let catalog = self.model.catalog();
                let table =
                    catalog.and_then(|c| c.find_table(f.ref_schema.as_deref(), &f.ref_table)).map(|(_, t)| t);
                let items = table
                    .map(|t| {
                        t.columns
                            .iter()
                            .map(|c| (c.name.clone(), format!("{}  {}", c.name, c.data_type)))
                            .collect()
                    })
                    .unwrap_or_default();
                (items, f.ref_columns.clone())
            }
            _ => {
                let d = self.model.draft();
                let chosen: Vec<u32> = match row {
                    RowId::Pk => d.primary_key.as_ref().map(|k| k.columns.clone()).unwrap_or_default(),
                    RowId::Unique(id) => {
                        d.uniques.iter().find(|k| k.id == id).map(|k| k.columns.clone()).unwrap_or_default()
                    }
                    RowId::Fk(id) => d
                        .foreign_keys
                        .iter()
                        .find(|k| k.id == id)
                        .map(|k| k.columns.clone())
                        .unwrap_or_default(),
                    RowId::Index(id) => {
                        d.indexes.iter().find(|k| k.id == id).map(|k| k.columns.clone()).unwrap_or_default()
                    }
                    _ => return,
                };
                let items = d
                    .columns
                    .iter()
                    .map(|c| (c.id.to_string(), format!("{}  {}", c.name, c.data_type)))
                    .collect();
                (items, chosen.iter().map(u32::to_string).collect())
            }
        };
        if items.is_empty() {
            self.say(true, "There are no columns to choose from yet", cx);
            return;
        }
        self.popup = Some(ListPopup { row, field, items, chosen, cursor: 0 });
        cx.notify();
    }

    /// Apply the popup's choice to the model (live: the preview follows each toggle).
    pub(crate) fn push_list(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.popup else { return };
        let (row, field, chosen) = (p.row, p.field, p.chosen.clone());
        if field == Field::FkRefColumns {
            if let RowId::Fk(id) = row {
                self.model.set_ref_columns(id, chosen);
            }
        } else {
            self.model.set_columns(row, chosen.iter().filter_map(|k| k.parse().ok()).collect());
        }
        self.after_edit(cx);
    }

    fn popup_key(&mut self, press: Press, cx: &mut Context<Self>) {
        let Some(p) = &mut self.popup else { return };
        let n = p.items.len();
        match (press.key, press.alt) {
            ("escape", _) | ("enter", _) => {
                self.popup = None;
                cx.notify();
            }
            ("up", false) | ("k", false) => p.cursor = (p.cursor + n - 1) % n,
            ("down", false) | ("j", false) => p.cursor = (p.cursor + 1) % n,
            ("space", _) | ("x", false) => {
                let key = p.items[p.cursor].0.clone();
                if let Some(at) = p.chosen.iter().position(|c| *c == key) {
                    p.chosen.remove(at);
                } else {
                    p.chosen.push(key);
                }
                self.push_list(cx);
            }
            // Reorder what is chosen: the item under the cursor swaps with its neighbour in the key order.
            ("up", true) | ("down", true) => {
                let key = p.items[p.cursor].0.clone();
                if let Some(at) = p.chosen.iter().position(|c| *c == key) {
                    let to = at as i32 + if press.key == "up" { -1 } else { 1 };
                    if to >= 0 && (to as usize) < p.chosen.len() {
                        p.chosen.swap(at, to as usize);
                        self.push_list(cx);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    // ------------------------------------------------------------- cursor

    fn move_cursor(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        let spec = self.model.spec(self.section);
        if spec.rows.is_empty() {
            return;
        }
        let (r, c) = self.clamped_cursor(&spec);
        let r2 = (r as i32 + dy).clamp(0, spec.rows.len() as i32 - 1) as usize;
        let c2 = (c as i32 + dx).clamp(0, spec.cols.len() as i32 - 1) as usize;
        self.cursors.insert(self.section, (r2, c2));
        self.scroll.scroll_to_item(r2, gpui_kit::ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Tab: the next cell, wrapping to the next row. With `edit`, a text cell opens for typing at once, so a
    /// whole row can be filled in with Tab and the keyboard alone.
    fn move_next(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>, edit: bool) {
        let spec = self.model.spec(self.section);
        if spec.rows.is_empty() {
            return;
        }
        let (r, c) = self.clamped_cursor(&spec);
        let width = spec.cols.len() as i32;
        let flat = (r as i32 * width + c as i32 + delta).clamp(0, spec.rows.len() as i32 * width - 1);
        let (r2, c2) = ((flat / width) as usize, (flat % width) as usize);
        self.cursors.insert(self.section, (r2, c2));
        self.scroll.scroll_to_item(r2, gpui_kit::ScrollStrategy::Nearest);
        if edit {
            let kind = spec.rows[r2].cells.iter().find(|x| x.field == spec.cols[c2].field).map(|x| x.kind);
            if matches!(kind, Some(CellKind::Text | CellKind::Type))
                && !matches!(spec.rows[r2].id, RowId::Removed(_))
            {
                self.activate(window, cx);
            }
        }
        cx.notify();
    }

    /// Apply a [`Cmd`].
    pub fn command(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Move { dx, dy } => self.move_cursor(dx, dy, cx),
            Cmd::Next => self.move_next(1, window, cx, false),
            Cmd::Prev => self.move_next(-1, window, cx, false),
            Cmd::Top => {
                self.cursors.entry(self.section).or_default().0 = 0;
                self.scroll.scroll_to_item(0, gpui_kit::ScrollStrategy::Nearest);
                cx.notify();
            }
            Cmd::Bottom => {
                let n = self.model.spec(self.section).rows.len();
                self.cursors.entry(self.section).or_default().0 = n.saturating_sub(1);
                self.scroll.scroll_to_item(n.saturating_sub(1), gpui_kit::ScrollStrategy::Nearest);
                cx.notify();
            }
            Cmd::Edit => self.activate(window, cx),
            Cmd::Flip(step) => {
                let spec = self.model.spec(self.section);
                let (r, c) = self.clamped_cursor(&spec);
                let Some(row) = spec.rows.get(r) else { return };
                let Some(cell) =
                    row.cells.iter().find(|x| Some(x.field) == spec.cols.get(c).map(|c| c.field))
                else {
                    return;
                };
                match cell.kind {
                    CellKind::Toggle if !self.read_only_notice(cx) => {
                        self.set_cell(row.id, cell.field, CellValue::Flip, cx);
                    }
                    CellKind::Cycle if !self.read_only_notice(cx) => {
                        self.set_cell(row.id, cell.field, CellValue::Step(step), cx);
                    }
                    CellKind::Columns | CellKind::RefTable => self.activate(window, cx),
                    _ => {}
                }
            }
            Cmd::AddBelow => self.add_row(false, window, cx),
            Cmd::AddAbove => self.add_row(true, window, cx),
            Cmd::Delete => self.delete_current(cx),
            Cmd::Duplicate => self.duplicate_current(cx),
            Cmd::MoveRow(d) => self.move_current(d, cx),
            Cmd::Undo => self.undo(cx),
            Cmd::Redo => self.redo(cx),
            Cmd::SectionNext => self.step_section(1, window, cx),
            Cmd::SectionPrev => self.step_section(-1, window, cx),
            Cmd::TogglePreview => {
                self.preview_open = !self.preview_open;
                cx.notify();
            }
            Cmd::CopySql => self.copy_script(cx),
            Cmd::OpenInConsole => self.open_in_console(cx),
            Cmd::Revert => self.revert(window, cx),
            Cmd::FocusName => self.focus_name(window, cx),
            Cmd::AddSuggested => {
                if self.section != Section::ForeignKeys {
                    self.switch_section(Section::ForeignKeys, window, cx);
                }
                if self.open_suggestions().is_empty() {
                    self.say(false, "No foreign keys are suggested for this table", cx);
                } else {
                    cx.emit(TableEditorEvent::PickSuggestion);
                }
            }
        }
    }

    pub(crate) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        let press = Press {
            key: ev.keystroke.key.as_str(),
            shift: m.shift,
            alt: m.alt,
            primary: m.control || m.platform,
        };
        if self.popup.is_some() {
            self.popup_key(press, cx);
            cx.stop_propagation();
            return;
        }
        // An edit whose input lost focus without telling us (focus moved by code) is over: apply it.
        if self.editing.is_some() && self.focus.is_focused(window) && !self.commit_edit_inner(window, cx) {
            self.cancel_edit(window, cx);
        }
        // Only act when the grid itself has focus: the name field and the cell editor are descendants
        // whose key events also bubble up here.
        if self.editing.is_some() || !self.focus.is_focused(window) {
            return;
        }
        let modal = Settings::get(cx).keymap.is_modal();
        let (cmd, pending) = keys::interpret(press, modal, self.pending);
        self.pending = pending;
        if let Some(cmd) = cmd {
            self.command(cmd, window, cx);
        }
        if cmd.is_some() || pending.is_some() {
            cx.stop_propagation();
        }
    }
}

/// Where the cursor goes after a cell edit is committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    Stay,
    Forward,
    Back,
}
