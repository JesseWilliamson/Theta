//! The dialog that asks what to compare: a connection and a schema for each side, and for a data comparison a
//! table for each side and the key.
//!
//! Everything it decides (which rows exist, what the arrow keys do, what is wrong with the choice) is in
//! [`Model`], a plain struct with unit tests; [`ComparePickView`] paints it and forwards keys. Every row is
//! reachable by keys alone: up / down (or `j` / `k`, tab) move, left / right (or `h` / `l`) change the value,
//! `/` types a prefix to jump to, space ticks a key column, enter compares, esc closes.

use crate::connections::ConnectionStore;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use std::rc::Rc;
use theta_core::{DatabaseKind, Table, TableKind};

/// What a connection offers: its schemas and, for each, the tables.
#[derive(Debug, Clone)]
pub struct Choice {
    pub conn_id: String,
    pub name: String,
    pub kind: DatabaseKind,
    pub schemas: Vec<SchemaChoice>,
}

#[derive(Debug, Clone)]
pub struct SchemaChoice {
    pub name: String,
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Two schemas.
    Schema,
    /// Two tables and the key that pairs their rows.
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub conn_id: String,
    pub schema: String,
    pub table: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub source: Side,
    pub target: Side,
    /// Columns that pair the rows; empty means "the primary key".
    pub key: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Connection(usize),
    Schema(usize),
    Table(usize),
    /// How rows are paired: the primary key, a unique constraint, or columns you choose.
    Key,
    /// One column of the "choose columns" checklist.
    KeyColumn(usize),
}

#[derive(Debug, Clone, Default)]
struct Sel {
    conn: usize,
    schema: usize,
    table: usize,
}

pub struct Model {
    pub mode: Mode,
    pub choices: Vec<Choice>,
    sel: [Sel; 2],
    pub cursor: usize,
    /// 0 = primary key, 1.. = unique constraints of the source table, last = choose columns.
    key_choice: usize,
    ticked: Vec<String>,
    /// Typed prefix while `/` is active.
    pub typing: Option<String>,
    /// The user has changed something: until then a missing choice is an instruction, not an error.
    pub touched: bool,
}

impl Model {
    /// `start` is the connection (and schema, table) to preselect for the source.
    pub fn new(mode: Mode, choices: Vec<Choice>, start: Option<&Side>) -> Model {
        let mut m = Model {
            mode,
            choices,
            sel: [Sel::default(), Sel::default()],
            cursor: 0,
            key_choice: 0,
            ticked: Vec::new(),
            typing: None,
            touched: false,
        };
        if let Some(s) = start
            && let Some(ci) = m.choices.iter().position(|c| c.conn_id == s.conn_id)
        {
            m.sel[0].conn = ci;
            m.sel[1].conn = ci;
            if let Some(si) =
                m.choices[ci].schemas.iter().position(|x| x.name.eq_ignore_ascii_case(&s.schema))
            {
                m.sel[0].schema = si;
                m.sel[1].schema = si;
            }
            if let Some(t) = &s.table
                && let Some(ti) = m.tables(0).iter().position(|x| x.name.eq_ignore_ascii_case(t))
            {
                m.sel[0].table = ti;
                m.sel[1].table = ti;
            }
        }
        // The target starts one step along from the source, so the dialog does not open on "compare a thing
        // with itself": another schema of the same connection, or in table mode another table.
        let n_schemas = m.choices.get(m.sel[1].conn).map_or(0, |c| c.schemas.len());
        match mode {
            Mode::Schema if n_schemas > 1 => m.sel[1].schema = (m.sel[0].schema + 1) % n_schemas,
            // Data is compared between two copies of one table, so the target starts on the same table: in
            // another schema (or connection) that has one of that name. Failing that it starts on the
            // source itself and the dialog says a second table or schema has to be chosen.
            Mode::Table => m.pair_same_table(),
            _ => {}
        }
        m
    }

    /// Point the target at a table with the source table's name in another schema, if there is one.
    fn pair_same_table(&mut self) {
        let Some(name) = self.table(0).map(|t| t.name.clone()) else { return };
        let conn = self.sel[1].conn;
        let current = self.sel[0].schema;
        let found = self.choices[conn].schemas.iter().enumerate().find_map(|(si, sc)| {
            let ti = sc
                .tables
                .iter()
                .filter(|t| t.kind == TableKind::Table)
                .position(|t| t.name.eq_ignore_ascii_case(&name))?;
            (si != current).then_some((si, ti))
        });
        if let Some((si, ti)) = found {
            self.sel[1].schema = si;
            self.sel[1].table = ti;
        }
    }

    fn schema_of(&self, side: usize) -> Option<&SchemaChoice> {
        self.choices.get(self.sel[side].conn)?.schemas.get(self.sel[side].schema)
    }

    /// The tables a side can pick: real tables only (views have no rows to key).
    pub fn tables(&self, side: usize) -> Vec<&Table> {
        self.schema_of(side)
            .map(|s| s.tables.iter().filter(|t| t.kind == TableKind::Table).collect())
            .unwrap_or_default()
    }

    pub fn table(&self, side: usize) -> Option<&Table> {
        self.tables(side).get(self.sel[side].table).copied()
    }

    pub fn rows(&self) -> Vec<RowKind> {
        let mut v = Vec::new();
        for side in 0..2 {
            v.push(RowKind::Connection(side));
            v.push(RowKind::Schema(side));
            if self.mode == Mode::Table {
                v.push(RowKind::Table(side));
            }
        }
        if self.mode == Mode::Table {
            v.push(RowKind::Key);
            if self.key_is_custom() {
                v.extend((0..self.common_columns().len()).map(RowKind::KeyColumn));
            }
        }
        v
    }

    /// Options of the primary key and unique constraints of the source table, then "choose columns".
    pub fn key_options(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(t) = self.table(0) {
            // A table without a primary key does not offer one: it would be a choice that can only fail.
            if !t.primary_key.is_empty() {
                v.push(format!("primary key ({})", t.primary_key.join(", ")));
            }
            for u in &t.unique_constraints {
                v.push(format!("unique {} ({})", u.name, u.columns.join(", ")));
            }
        }
        v.push("choose columns".to_string());
        v
    }

    /// Whether the first key option is the source table's primary key.
    fn has_pk(&self) -> bool {
        self.table(0).is_some_and(|t| !t.primary_key.is_empty())
    }

    fn key_is_custom(&self) -> bool {
        self.key_choice + 1 == self.key_options().len()
    }

    /// Column names both tables have (case-insensitive), in the source's order.
    pub fn common_columns(&self) -> Vec<String> {
        let (Some(a), Some(b)) = (self.table(0), self.table(1)) else { return Vec::new() };
        a.columns
            .iter()
            .filter(|c| b.columns.iter().any(|d| d.name.eq_ignore_ascii_case(&c.name)))
            .map(|c| c.name.clone())
            .collect()
    }

    pub fn is_ticked(&self, name: &str) -> bool {
        self.ticked.iter().any(|t| t.eq_ignore_ascii_case(name))
    }

    pub fn step(&mut self, by: isize) {
        let n = self.rows().len() as isize;
        self.cursor = (self.cursor as isize + by).rem_euclid(n.max(1)) as usize;
        self.typing = None;
    }

    fn clamp_cursor(&mut self) {
        let n = self.rows().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    /// The options of the row under the cursor and the index chosen.
    pub fn options(&self, row: RowKind) -> (Vec<String>, usize) {
        match row {
            RowKind::Connection(s) => (
                self.choices.iter().map(|c| format!("{} · {}", c.name, c.kind.label())).collect(),
                self.sel[s].conn,
            ),
            RowKind::Schema(s) => (
                self.choices
                    .get(self.sel[s].conn)
                    .map(|c| c.schemas.iter().map(|x| x.name.clone()).collect())
                    .unwrap_or_default(),
                self.sel[s].schema,
            ),
            RowKind::Table(s) => (self.tables(s).iter().map(|t| t.name.clone()).collect(), self.sel[s].table),
            RowKind::Key => (self.key_options(), self.key_choice),
            RowKind::KeyColumn(_) => (Vec::new(), 0),
        }
    }

    /// Change the value of the current row by `by` options (wrapping), or tick a key column.
    pub fn change(&mut self, by: isize) {
        let Some(row) = self.rows().get(self.cursor).copied() else { return };
        let (opts, at) = self.options(row);
        if opts.is_empty() {
            return;
        }
        let to = (at as isize + by).rem_euclid(opts.len() as isize) as usize;
        self.set(row, to);
    }

    pub fn set(&mut self, row: RowKind, to: usize) {
        self.touched = true;
        // What was chosen below the row that changes is kept when the new parent has the same thing:
        // switching schema keeps the table of that name, switching connection keeps schema and table.
        let (keep_schema, keep_table) = match row {
            RowKind::Connection(s) => (
                self.schema_of(s).map(|x| x.name.clone()),
                self.table(s).map(|t| t.name.clone()),
            ),
            RowKind::Schema(s) => (None, self.table(s).map(|t| t.name.clone())),
            _ => (None, None),
        };
        match row {
            RowKind::Connection(s) => {
                self.sel[s] = Sel { conn: to, ..Sel::default() };
                if let Some(name) = keep_schema
                    && let Some(i) = self.choices[to].schemas.iter().position(|x| x.name.eq_ignore_ascii_case(&name))
                {
                    self.sel[s].schema = i;
                }
                self.keep_table(s, keep_table);
            }
            RowKind::Schema(s) => {
                self.sel[s].schema = to;
                self.sel[s].table = 0;
                self.keep_table(s, keep_table);
            }
            RowKind::Table(s) => self.sel[s].table = to,
            RowKind::Key => self.key_choice = to,
            RowKind::KeyColumn(_) => {}
        }
        // Columns that stopped existing cannot stay ticked.
        let common = self.common_columns();
        self.ticked.retain(|t| common.iter().any(|c| c.eq_ignore_ascii_case(t)));
        let n = self.key_options().len();
        self.key_choice = self.key_choice.min(n.saturating_sub(1));
        self.clamp_cursor();
    }

    fn keep_table(&mut self, side: usize, name: Option<String>) {
        if let Some(name) = name
            && let Some(i) = self.tables(side).iter().position(|t| t.name.eq_ignore_ascii_case(&name))
        {
            self.sel[side].table = i;
        }
    }

    /// Jump the current row to the first option that starts with `prefix` (case-insensitive).
    pub fn jump_to_prefix(&mut self, prefix: &str) {
        let Some(row) = self.rows().get(self.cursor).copied() else { return };
        let (opts, _) = self.options(row);
        if let Some(i) = opts.iter().position(|o| o.to_lowercase().starts_with(&prefix.to_lowercase())) {
            self.set(row, i);
        }
    }

    pub fn toggle(&mut self) {
        let Some(RowKind::KeyColumn(i)) = self.rows().get(self.cursor).copied() else { return };
        self.touched = true;
        let Some(name) = self.common_columns().get(i).cloned() else { return };
        if let Some(p) = self.ticked.iter().position(|t| t.eq_ignore_ascii_case(&name)) {
            self.ticked.remove(p);
        } else {
            self.ticked.push(name);
        }
    }

    pub fn side(&self, side: usize) -> Option<Side> {
        let c = self.choices.get(self.sel[side].conn)?;
        let s = c.schemas.get(self.sel[side].schema)?;
        Some(Side {
            conn_id: c.conn_id.clone(),
            schema: s.name.clone(),
            table: (self.mode == Mode::Table).then(|| self.table(side).map(|t| t.name.clone())).flatten(),
        })
    }

    /// Why the choice cannot be compared, or `None`.
    pub fn problem(&self) -> Option<String> {
        let (a, b) = (self.side(0), self.side(1));
        let (Some(a), Some(b)) = (a, b) else {
            return Some("There is nothing to choose from: connect first".into());
        };
        if a == b {
            // Not a mistake before the user has done anything: say what is needed.
            let one_schema = self.choices.get(self.sel[0].conn).is_some_and(|c| c.schemas.len() == 1);
            return Some(match (self.mode, self.touched) {
                (Mode::Schema, false) if one_schema => {
                    "Choose another schema or another connection as the target. This connection has one schema.".into()
                }
                (Mode::Schema, false) => "Choose a different schema or connection as the target.".into(),
                (Mode::Table, false) => "Choose a different table, schema or connection as the target.".into(),
                (Mode::Schema, true) => "The source and the target are the same schema".into(),
                (Mode::Table, true) => "The source and the target are the same table".into(),
            });
        }
        if self.mode == Mode::Table {
            if a.table.is_none() || b.table.is_none() {
                return Some("A schema without tables cannot be compared".into());
            }
            if self.key_is_custom() && self.ticked.is_empty() {
                return Some("Tick at least one key column (space)".into());
            }
        }
        None
    }

    /// A caution that does not block: what will be different about the result.
    pub fn caution(&self) -> Option<String> {
        let kinds = (self.choices.get(self.sel[0].conn)?.kind, self.choices.get(self.sel[1].conn)?.kind);
        (kinds.0 != kinds.1 && self.mode == Mode::Schema).then(|| {
            format!(
                "{} and {} are different products: the differences will be shown, but no migration script can be generated.",
                kinds.0.label(),
                kinds.1.label()
            )
        })
    }

    pub fn picked(&self) -> Option<Picked> {
        if self.problem().is_some() {
            return None;
        }
        let key = if self.key_is_custom() {
            self.common_columns().into_iter().filter(|c| self.is_ticked(c)).collect()
        } else if self.has_pk() && self.key_choice == 0 {
            Vec::new()
        } else {
            let unique = self.key_choice - usize::from(self.has_pk());
            self.table(0)
                .and_then(|t| t.unique_constraints.get(unique))
                .map(|u| u.columns.clone())
                .unwrap_or_default()
        };
        Some(Picked { source: self.side(0)?, target: self.side(1)?, key })
    }
}

/// The connected connections with a loaded catalog, as choices.
pub fn choices(store: &Entity<ConnectionStore>, cx: &App) -> Vec<Choice> {
    let s = store.read(cx);
    s.configs()
        .iter()
        .filter_map(|c| {
            let cat = s.catalog(&c.id)?;
            let mut schemas: Vec<SchemaChoice> = cat
                .schemas
                .iter()
                .filter(|x| !x.tables.is_empty() || !x.routines.is_empty())
                .map(|x| SchemaChoice { name: x.name.clone(), tables: x.tables.clone() })
                .collect();
            schemas.sort_by_key(|x| (crate::workspace::is_system_schema(&x.name), x.name.to_lowercase()));
            (!schemas.is_empty()).then(|| Choice {
                conn_id: c.id.clone(),
                name: s.label(&c.id).unwrap_or_else(|| c.name.clone()),
                kind: c.kind,
                schemas,
            })
        })
        .collect()
}

pub type Done = Rc<dyn Fn(Picked, &mut Window, &mut App)>;

pub struct ComparePickView {
    model: Model,
    focus: FocusHandle,
    done: Done,
    action: &'static str,
    error: Option<String>,
}

pub fn open(
    title: &'static str,
    action: &'static str,
    model: Model,
    done: Done,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| ComparePickView { model, focus: cx.focus_handle(), done, action, error: None });
    let focus = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .bg(cx.theme().popover)
            .title(title)
            .w(px(620.))
            .overlay_closable(true)
            // Enter is the dialog's OK; `go` closes the dialog itself, and only when the choice is valid.
            .on_ok({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |v, cx| v.go(window, cx));
                    false
                }
            })
            .child(view.clone())
    });
    // Without this the editor behind the dialog keeps the focus and the rows cannot be reached by keys.
    window.defer(cx, move |window, cx| {
        let h = focus.read(cx).focus.clone();
        window.focus(&h, cx);
    });
}

impl ComparePickView {
    fn go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The dialog's own Enter binding runs before the key handler: while a name is being typed, Enter
        // only ends the typing.
        if self.model.typing.take().is_some() {
            cx.notify();
            return;
        }
        match self.model.picked() {
            Some(p) => {
                let done = self.done.clone();
                window.close_dialog(cx);
                done(p, window, cx);
            }
            None => {
                self.error = self.model.problem();
                cx.notify();
            }
        }
    }
}

fn label(row: RowKind, mode: Mode) -> String {
    let side = |s: usize| if s == 0 { "Source" } else { "Target" };
    match row {
        RowKind::Connection(s) => format!("{} connection", side(s)),
        RowKind::Schema(s) => format!("{} schema", side(s)),
        RowKind::Table(s) => format!("{} table", side(s)),
        RowKind::Key => (if mode == Mode::Table { "Rows are paired by" } else { "" }).to_string(),
        RowKind::KeyColumn(_) => String::new(),
    }
}

impl Render for ComparePickView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus.is_focused(window);
        let rows = self.model.rows();
        let cursor = self.model.cursor;
        let problem = self.model.problem();
        let common = self.model.common_columns();
        let mode = self.model.mode;
        let typing = self.model.typing.clone();
        let hint = if typing.is_some() {
            "type a name · enter stops typing · esc closes"
        } else if mode == Mode::Table {
            "up / down move · left / right change · / type a name · space ticks a column · enter compares · esc closes"
        } else {
            "up / down move · left / right change · / type a name · enter compares · esc closes"
        };
        let body = v_flex()
            .gap_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let m = ev.keystroke.modifiers;
                if m.control || m.platform || m.alt {
                    return;
                }
                let key = ev.keystroke.key.as_str();
                if let Some(buf) = this.model.typing.clone() {
                    match key {
                        "escape" | "enter" => this.model.typing = None,
                        "backspace" => {
                            let mut b = buf;
                            b.pop();
                            this.model.jump_to_prefix(&b);
                            this.model.typing = Some(b);
                        }
                        _ => {
                            if let Some(t) =
                                ev.keystroke.key_char.as_deref().filter(|t| !t.chars().any(char::is_control))
                            {
                                let b = format!("{buf}{t}");
                                this.model.jump_to_prefix(&b);
                                this.model.typing = Some(b);
                            }
                        }
                    }
                    this.error = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                match key {
                    "down" | "j" | "tab" if !m.shift => this.model.step(1),
                    "up" | "k" | "tab" => this.model.step(-1),
                    "left" | "h" => this.model.change(-1),
                    "right" | "l" => this.model.change(1),
                    "pagedown" => this.model.change(10),
                    "pageup" => this.model.change(-10),
                    "space" => this.model.toggle(),
                    "/" => this.model.typing = Some(String::new()),
                    "enter" => this.go(window, cx),
                    _ => return,
                }
                this.error = None;
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(hint))
            .child(v_flex().border_1().border_color(theme.border).rounded_md().children(
                rows.iter().enumerate().map(|(i, row)| {
                    let (opts, at) = self.model.options(*row);
                    let value = match row {
                        RowKind::KeyColumn(c) => {
                            let name = common.get(*c).cloned().unwrap_or_default();
                            format!("{} {name}", if self.model.is_ticked(&name) { "☑" } else { "☐" })
                        }
                        _ => opts.get(at).cloned().unwrap_or_else(|| "(none)".to_string()),
                    };
                    let count =
                        if opts.len() > 1 { format!("{} of {}", at + 1, opts.len()) } else { String::new() };
                    let is_col = matches!(row, RowKind::KeyColumn(_));
                    h_flex()
                        .id(("pick-row", i))
                        .h(px(28.))
                        .px_2()
                        .gap_2()
                        .items_center()
                        .text_size(px(12.))
                        .when(focused && i == cursor, |d| d.bg(theme.list_active))
                        .when(matches!(row, RowKind::Connection(1) | RowKind::Key) && i > 0, |d| {
                            d.border_t_1().border_color(theme.border)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.model.cursor = i;
                            if matches!(this.model.rows().get(i), Some(RowKind::KeyColumn(_))) {
                                this.model.toggle();
                            }
                            cx.notify();
                        }))
                        .child(div().w(px(130.)).text_color(theme.muted_foreground).child(label(*row, mode)))
                        .when(!is_col, |d| {
                            d.child(div().w(px(10.)).text_color(theme.muted_foreground).child("‹"))
                        })
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .pl(if is_col { px(24.) } else { px(0.) })
                                .child(if i == cursor && typing.is_some() {
                                    format!("{value}   /{}▏", typing.clone().unwrap_or_default())
                                } else {
                                    value
                                }),
                        )
                        .when(!is_col, |d| {
                            d.child(div().text_color(theme.muted_foreground).child(count))
                                .child(div().w(px(10.)).text_color(theme.muted_foreground).child("›"))
                        })
                }),
            ))
            .when_some(self.model.caution(), |d, c| {
                d.child(div().text_size(px(11.)).text_color(theme.warning).child(c))
            })
            .when_some(self.error.clone().or(problem.clone()), |d, e| {
                // An instruction until the user has changed something, an error afterwards.
                let colour = if self.error.is_some() || self.model.touched {
                    theme.danger
                } else {
                    theme.muted_foreground
                };
                d.child(div().text_size(px(11.)).text_color(colour).child(e))
            })
;
        let footer = 
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("pick-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)),
                    )
                    .child(
                        Button::new("pick-go")
                            .primary()
                            .label(self.action)
                            .disabled(problem.is_some())
                            .on_click(cx.listener(|this, _, w, cx| this.go(w, cx))),
                    );
        super::framed(window, body, footer, 150.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theta_core::{ColumnInfo, UniqueConstraint};

    fn table(name: &str, cols: &[&str], pk: &[&str]) -> Table {
        Table {
            name: name.into(),
            columns: cols.iter().map(|c| ColumnInfo { name: c.to_string(), ..Default::default() }).collect(),
            primary_key: pk.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn choices() -> Vec<Choice> {
        vec![
            Choice {
                conn_id: "a".into(),
                name: "alpha".into(),
                kind: DatabaseKind::Postgres,
                schemas: vec![
                    SchemaChoice {
                        name: "public".into(),
                        tables: vec![table("t1", &["id", "x"], &["id"]), table("t2", &["id", "y"], &[])],
                    },
                    SchemaChoice { name: "staging".into(), tables: vec![table("t1", &["id", "x"], &["id"])] },
                ],
            },
            Choice {
                conn_id: "b".into(),
                name: "beta".into(),
                kind: DatabaseKind::MySql,
                schemas: vec![SchemaChoice {
                    name: "app".into(),
                    tables: vec![table("t1", &["ID"], &["ID"])],
                }],
            },
        ]
    }

    fn start() -> Side {
        Side { conn_id: "a".into(), schema: "public".into(), table: None }
    }

    #[test]
    fn a_schema_dialog_opens_on_the_active_connection_and_a_different_target_schema() {
        let m = Model::new(Mode::Schema, choices(), Some(&start()));
        assert_eq!(m.side(0).unwrap().schema, "public");
        assert_eq!(m.side(1).unwrap().schema, "staging", "the target is not the source");
        assert!(m.problem().is_none());
        assert_eq!(m.rows().len(), 4);
    }

    #[test]
    fn comparing_a_schema_with_itself_is_refused() {
        let mut m = Model::new(Mode::Schema, choices(), Some(&start()));
        m.cursor = 3; // target schema
        m.change(1);
        assert_eq!(m.side(1).unwrap().schema, "public");
        assert_eq!(m.problem().as_deref(), Some("The source and the target are the same schema"));
        assert!(m.picked().is_none());
    }

    #[test]
    fn changing_a_connection_resets_what_depends_on_it() {
        let mut m = Model::new(Mode::Schema, choices(), Some(&start()));
        m.cursor = 2; // target connection
        m.change(1);
        assert_eq!(m.side(1).unwrap().conn_id, "b");
        assert_eq!(m.side(1).unwrap().schema, "app");
        assert!(m.caution().unwrap().contains("different products"));
        assert!(m.problem().is_none(), "different products may be compared, only the script is off");
    }

    #[test]
    fn options_wrap_around() {
        let mut m = Model::new(Mode::Schema, choices(), Some(&start()));
        m.cursor = 1;
        m.change(-1);
        assert_eq!(m.side(0).unwrap().schema, "staging");
        m.change(1);
        assert_eq!(m.side(0).unwrap().schema, "public");
    }

    #[test]
    fn typing_a_prefix_jumps_within_the_row() {
        let mut m = Model::new(Mode::Schema, choices(), Some(&start()));
        m.cursor = 1;
        m.jump_to_prefix("ST");
        assert_eq!(m.side(0).unwrap().schema, "staging");
        m.jump_to_prefix("zzz");
        assert_eq!(m.side(0).unwrap().schema, "staging", "no match leaves it");
    }

    #[test]
    fn a_table_dialog_has_table_rows_and_a_key() {
        let mut m = Model::new(Mode::Table, choices(), Some(&Side { table: Some("t1".into()), ..start() }));
        assert_eq!(m.rows().len(), 7);
        assert_eq!(m.side(0).unwrap().table.as_deref(), Some("t1"));
        assert_eq!(m.side(1).unwrap().table.as_deref(), Some("t1"), "the target starts on the same table");
        assert_eq!(m.side(1).unwrap().schema, "staging", "in the other schema that has it");
        assert_eq!(m.key_options(), ["primary key (id)", "choose columns"]);
        assert!(m.problem().is_none());
        m.cursor = 6;
        m.change(1);
        assert_eq!(m.rows().len(), 7 + 2, "the checklist appears: both tables have `id` and `x`");
        assert_eq!(m.problem().as_deref(), Some("Tick at least one key column (space)"));
        m.step(1);
        m.toggle();
        assert_eq!(m.picked().unwrap().key, ["id"]);
    }

    #[test]
    fn a_source_table_without_a_key_needs_columns_or_a_unique_constraint() {
        let mut ch = choices();
        ch[0].schemas[0].tables[1].unique_constraints =
            vec![UniqueConstraint { name: "uq".into(), columns: vec!["y".into()] }];
        let mut m = Model::new(Mode::Table, ch, Some(&Side { table: Some("t2".into()), ..start() }));
        // Source t2 has no primary key, so none is offered: its unique constraint is, and choosing columns.
        assert_eq!(m.key_options(), ["unique uq (y)", "choose columns"]);
        // No other schema has a t2: the target starts on the source and the dialog asks for a second table,
        // in words, not as a mistake.
        assert!(m.problem().unwrap().starts_with("Choose a different table"));
        assert!(!m.touched);
        m.cursor = 5; // target table
        m.change(1);
        assert_eq!(m.side(1).unwrap().table.as_deref(), Some("t1"));
        assert_eq!(m.picked().unwrap().key, ["y"]);
    }

    #[test]
    fn changing_the_schema_keeps_the_table_of_that_name() {
        let mut m = Model::new(Mode::Table, choices(), Some(&Side { table: Some("t1".into()), ..start() }));
        m.cursor = 4; // target schema
        m.change(-1); // back to public
        m.change(1); // and to staging again
        assert_eq!(m.side(1).unwrap().schema, "staging");
        assert_eq!(m.side(1).unwrap().table.as_deref(), Some("t1"));
    }

    #[test]
    fn an_untouched_same_schema_dialog_explains_instead_of_complaining() {
        let m = Model::new(
            Mode::Schema,
            vec![Choice {
                conn_id: "a".into(),
                name: "lite".into(),
                kind: DatabaseKind::Sqlite,
                schemas: vec![SchemaChoice { name: "main".into(), tables: vec![] }],
            }],
            None,
        );
        assert!(!m.touched);
        assert!(m.problem().unwrap().contains("another connection"));
    }

    #[test]
    fn common_columns_match_without_regard_to_case() {
        let mut m = Model::new(Mode::Table, choices(), Some(&Side { table: Some("t1".into()), ..start() }));
        m.cursor = 0;
        m.change(1); // source connection -> beta (t1 with ID)
        m.cursor = 1;
        assert_eq!(m.common_columns(), ["ID"]);
    }

    #[test]
    fn views_are_not_offered_as_tables() {
        let mut ch = choices();
        ch[0].schemas[0].tables.push(Table { name: "v".into(), kind: TableKind::View, ..Default::default() });
        let m = Model::new(Mode::Table, ch, Some(&start()));
        assert_eq!(m.tables(0).len(), 2);
    }
}
