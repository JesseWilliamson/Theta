//! The editor's code intelligence as workspace commands: go to definition, find usages, rename, quick fixes
//! and the Problems tab. The analysis itself is `exec::intel` (pure and tested there); this file turns its
//! answers into cursor moves, pickers, dialogs and edits.
//!
//! Keys (the editor engine turns them into `Effect`s; see KEYBINDINGS.md):
//! Helix `gd` `gr` `space a` `space n` `space D` `[d` `]d`, Vim the same, Standard F12 shift-F12 ctrl-. F2
//! ctrl-shift-m F8 shift-F8.

use super::{BottomTab, NoticeKind, Workspace};
use crate::exec::intel::{self, Action, UsageTarget, Where};
use crate::panels::problems::ProblemsEvent;
use gpui_kit::assets::IconName;
use gpui_kit::{App, Context, Focusable as _, SharedString, WeakEntity, Window};
use std::ops::Range;
use std::sync::Arc;
use theta_core::{Catalog, DatabaseKind};
use theta_sql::assist::FixKind;
use theta_ui::picker::{PickerDelegate, PickerItem};

/// What the commands need to know about the console they act on.
pub(super) struct IntelCtx {
    pub text: String,
    pub cursor: usize,
    pub kind: DatabaseKind,
    pub catalog: Option<Arc<Catalog>>,
    pub connection: Option<String>,
}

impl Workspace {
    pub(super) fn intel_ctx(&self, id: u64, cx: &App) -> Option<IntelCtx> {
        let c = self.consoles.get(&id)?;
        let editor = c.editor.read(cx);
        Some(IntelCtx {
            text: editor.text(),
            cursor: editor.cursor_byte(),
            kind: self.console_kind(id, cx),
            catalog: c.catalog.read().ok().and_then(|c| c.clone()),
            connection: self.tabs.iter().find(|t| t.id == id).and_then(|t| t.connection_id.clone()),
        })
    }

    fn say(&mut self, id: u64, msg: impl Into<String>, cx: &mut Context<Self>) {
        let msg = msg.into();
        if let Some(c) = self.consoles.get(&id) {
            c.editor.update(cx, |e, cx| e.set_message(msg, cx));
        }
    }

    fn goto_in(&mut self, id: u64, byte: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_tab(id, window, cx);
        if let Some(c) = self.consoles.get(&id) {
            c.editor.update(cx, |e, cx| e.goto_byte(byte, cx));
            let handle = c.editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    // ---- go to definition

    pub(super) fn go_to_definition(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.intel_ctx(id, cx) else { return };
        match intel::definition(&ctx.text, ctx.cursor, ctx.kind, ctx.catalog.as_deref()) {
            Some(Where::Here(r)) => {
                let line = ctx.text[..r.start].matches('\n').count() + 1;
                self.goto_in(id, r.start, window, cx);
                self.say(id, format!("defined on line {line}"), cx);
            }
            Some(Where::Table(t)) => {
                self.show_table(id, &ctx, t.schema.as_deref(), &t.name, None, window, cx)
            }
            Some(Where::Column { table, column }) => {
                self.show_table(id, &ctx, table.schema.as_deref(), &table.name, Some(column), window, cx)
            }
            Some(Where::Function(f)) => {
                self.say(id, format!("'{f}' is a function; it has no definition in this script"), cx)
            }
            None => self.say(id, "No definition found at the cursor", cx),
        }
    }

    /// Reveal a catalog table in the explorer and peek at its columns.
    #[allow(clippy::too_many_arguments)]
    fn show_table(
        &mut self,
        id: u64,
        ctx: &IntelCtx,
        schema: Option<&str>,
        table: &str,
        column: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(conn), Some(cat)) = (ctx.connection.clone(), ctx.catalog.clone()) else { return };
        let Some((sch, t)) = cat.find_table(schema, table) else { return };
        let (sch_name, t) = (sch.name.clone(), t.clone());
        if !t.name.is_empty() {
            self.explorer.update(cx, |e, cx| e.reveal_table(&conn, &sch_name, &t.name, cx));
        }
        let delegate = PeekPicker::new(&sch_name, &t, column, cx.entity().downgrade(), id);
        self.open_picker(delegate, window, cx);
    }

    // ---- find usages

    pub(super) fn find_usages(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.intel_ctx(id, cx) else { return };
        let Some(target) = intel::usage_target(&ctx.text, ctx.cursor, ctx.kind, ctx.catalog.as_deref())
        else {
            self.say(id, "No name to find usages of at the cursor", cx);
            return;
        };
        let mut entries: Vec<UsageEntry> = Vec::new();
        let (title, place) = match target {
            UsageTarget::Local { name, ranges } => {
                for r in ranges {
                    entries.push(UsageEntry::new(id, String::new(), &ctx.text, r));
                }
                (format!("Usages of {name}"), false)
            }
            UsageTarget::Catalog(t) => {
                // every console of the same connection (and every .sql tab among them)
                let ids: Vec<u64> = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.connection_id == ctx.connection)
                    .map(|tab| tab.id)
                    .collect();
                for tid in ids {
                    let Some(other) = self.intel_ctx(tid, cx) else { continue };
                    let title =
                        self.tabs.iter().find(|x| x.id == tid).map(|x| x.title.clone()).unwrap_or_default();
                    for r in intel::catalog_usages(&other.text, other.kind, other.catalog.as_deref(), &t) {
                        entries.push(UsageEntry::new(tid, title.to_string(), &other.text, r));
                    }
                }
                let name = match &t {
                    theta_sql::assist::Target::Table(t) => t.name.clone(),
                    theta_sql::assist::Target::Column { table, column } => format!("{}.{column}", table.name),
                    _ => String::new(),
                };
                (format!("Usages of {name}"), true)
            }
        };
        if entries.is_empty() {
            self.say(id, "No usages found", cx);
            return;
        }
        let delegate = UsagesPicker::new(title, entries, place, cx.entity().downgrade());
        self.open_picker(delegate, window, cx);
    }

    // ---- code actions

    pub(super) fn code_actions(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.intel_ctx(id, cx) else { return };
        let actions = intel::code_actions(&ctx.text, ctx.cursor, ctx.kind, ctx.catalog.as_deref());
        if actions.is_empty() {
            self.say(id, "No actions available here", cx);
            return;
        }
        let delegate = ActionsPicker { actions, workspace: cx.entity().downgrade(), console: id };
        self.open_picker(delegate, window, cx);
    }

    pub(crate) fn apply_edits_to(
        &mut self,
        id: u64,
        edits: Vec<(Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(c) = self.consoles.get(&id) {
            c.editor.update(cx, |e, cx| e.replace_byte_ranges(edits, cx));
        }
    }

    // ---- rename

    pub(super) fn rename_symbol(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.intel_ctx(id, cx) else { return };
        let current = match intel::rename_prepare(&ctx.text, ctx.cursor, ctx.kind, ctx.catalog.as_deref()) {
            Ok(n) => n,
            Err(reason) => {
                self.notify(NoticeKind::Info, reason.clone(), window, cx);
                self.say(id, reason, cx);
                return;
            }
        };
        #[cfg(test)]
        if self.quiet {
            self.rename_asked.push((id, current));
            return;
        }
        let ws = cx.entity().downgrade();
        let (text, cursor, kind, catalog) = (ctx.text.clone(), ctx.cursor, ctx.kind, ctx.catalog.clone());
        let validate: crate::dialogs::name::Validate = std::rc::Rc::new(move |new, _| {
            intel::rename(&text, cursor, kind, catalog.as_deref(), new).map(|_| ())
        });
        let submit: crate::dialogs::name::Submit = std::rc::Rc::new(move |new, _window, cx| {
            ws.update(cx, |this, cx| {
                if let Err(e) = this.rename_with(id, &new, cx) {
                    this.say(id, e, cx);
                }
            })
            .ok();
        });
        crate::dialogs::name::open(
            crate::dialogs::name::Prompt {
                title: "Rename",
                label: &format!(
                    "New name for '{current}' (every use in this statement changes; tables and columns of the database are renamed in the table editor)"
                ),
                initial: &current,
                submit: "Rename",
            },
            validate,
            submit,
            window,
            cx,
        );
    }

    /// Rename the name under the cursor of console `id`: one undo step, or the reason it cannot be done.
    pub(crate) fn rename_with(
        &mut self,
        id: u64,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let ctx = self.intel_ctx(id, cx).ok_or("No such console")?;
        let edits = intel::rename(&ctx.text, ctx.cursor, ctx.kind, ctx.catalog.as_deref(), new_name)?;
        let n = edits.len();
        self.apply_edits_to(id, edits, cx);
        self.say(id, format!("renamed {n} occurrence{}", if n == 1 { "" } else { "s" }), cx);
        Ok(())
    }

    // ---- problems

    /// Refresh the Problems tab from the active console's editor.
    pub(super) fn refresh_problems(&mut self, cx: &mut Context<Self>) {
        let list = self
            .active_tab
            .and_then(|id| self.consoles.get(&id))
            .map(|c| c.editor.read(cx).problems())
            .unwrap_or_default();
        self.problems.update(cx, |p, cx| p.set(list, cx));
    }

    pub(super) fn show_problems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_problems(cx);
        self.state.bottom_open = true;
        self.set_bottom_tab(BottomTab::Problems, cx);
        let handle = self.problems.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub(super) fn on_problems_event(
        &mut self,
        e: ProblemsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.active_tab else { return };
        match e {
            ProblemsEvent::Jump(byte) => self.goto_in(id, byte, window, cx),
            ProblemsEvent::Leave => {
                if let Some(c) = self.consoles.get(&id) {
                    let handle = c.editor.read(cx).focus_handle(cx);
                    window.focus(&handle, cx);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- pickers

/// One place a name is used, shown as `line: text` (and the tab, for usages across tabs).
pub(super) struct UsageEntry {
    console: u64,
    byte: usize,
    label: String,
    tab: String,
}

impl UsageEntry {
    fn new(console: u64, tab: String, text: &str, r: Range<usize>) -> UsageEntry {
        let line = text[..r.start].matches('\n').count() + 1;
        let line_start = text[..r.start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[r.start..].find('\n').map_or(text.len(), |i| r.start + i);
        let shown = text[line_start..line_end].trim().to_string();
        UsageEntry { console, byte: r.start, label: format!("{line}: {shown}"), tab }
    }
}

pub(super) struct UsagesPicker {
    title: String,
    entries: Vec<UsageEntry>,
    show_tab: bool,
    workspace: WeakEntity<Workspace>,
}

impl UsagesPicker {
    fn new(
        title: String,
        entries: Vec<UsageEntry>,
        show_tab: bool,
        workspace: WeakEntity<Workspace>,
    ) -> Self {
        UsagesPicker { title, entries, show_tab, workspace }
    }
}

impl PickerDelegate for UsagesPicker {
    fn placeholder(&self) -> SharedString {
        format!("{} ({})", self.title, self.entries.len()).into()
    }

    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let item = PickerItem::new(i.to_string(), e.label.clone()).icon(IconName::Search);
                if self.show_tab && !e.tab.is_empty() { item.detail(e.tab.clone()) } else { item }
            })
            .collect()
    }

    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Ok(i) = item.id.parse::<usize>() else { return };
        let Some(e) = self.entries.get(i) else { return };
        let (console, byte) = (e.console, e.byte);
        let ws = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            ws.update(cx, |this, cx| this.goto_in(console, byte, window, cx)).ok();
        });
    }
}

pub(super) struct ActionsPicker {
    actions: Vec<Action>,
    workspace: WeakEntity<Workspace>,
    console: u64,
}

impl PickerDelegate for ActionsPicker {
    fn placeholder(&self) -> SharedString {
        "Quick fixes and refactorings…".into()
    }

    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.actions
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (section, icon) = match a.kind {
                    FixKind::QuickFix => ("Quick fixes", IconName::Check),
                    FixKind::Refactor => ("Refactor", IconName::Replace),
                    FixKind::Source => ("Source", IconName::CaseSensitive),
                };
                PickerItem::new(i.to_string(), a.title.clone()).section(section).icon(icon)
            })
            .collect()
    }

    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Ok(i) = item.id.parse::<usize>() else { return };
        let Some(a) = self.actions.get(i) else { return };
        let (edits, console) = (a.edits.clone(), self.console);
        let ws = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            ws.update(cx, |this, cx| {
                this.apply_edits_to(console, edits, cx);
                this.goto_in(console, this.intel_ctx(console, cx).map_or(0, |c| c.cursor), window, cx);
            })
            .ok();
        });
    }
}

/// The columns of a table, shown where the table was asked for: the peek of go to definition.
pub(super) struct PeekPicker {
    title: String,
    rows: Vec<(String, String)>,
    selected: Option<usize>,
    workspace: WeakEntity<Workspace>,
}

impl PeekPicker {
    fn new(
        schema: &str,
        table: &theta_core::Table,
        column: Option<String>,
        workspace: WeakEntity<Workspace>,
        _console: u64,
    ) -> Self {
        let rows: Vec<(String, String)> = table
            .columns
            .iter()
            .map(|c| {
                let pk = if table.primary_key.iter().any(|k| k == &c.name) { " · primary key" } else { "" };
                let null = if c.nullable { "" } else { " · not null" };
                (c.name.clone(), format!("{}{null}{pk}", c.data_type))
            })
            .collect();
        let selected = column.and_then(|c| rows.iter().position(|(n, _)| n.eq_ignore_ascii_case(&c)));
        let kind = match table.kind {
            theta_core::TableKind::Table => "table",
            theta_core::TableKind::View => "view",
            theta_core::TableKind::MaterializedView => "materialized view",
            theta_core::TableKind::ForeignTable => "foreign table",
        };
        PeekPicker {
            title: format!("{schema}.{} · {kind} · {} columns", table.name, rows.len()),
            rows,
            selected,
            workspace,
        }
    }
}

impl PickerDelegate for PeekPicker {
    fn placeholder(&self) -> SharedString {
        self.title.clone().into()
    }

    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.rows
            .iter()
            .enumerate()
            .map(|(i, (name, ty))| {
                PickerItem::new(i.to_string(), name.clone()).detail(ty.clone()).icon(IconName::Table2)
            })
            .collect()
    }

    fn initial_selection(&self, _items: &[PickerItem]) -> Option<usize> {
        self.selected
    }

    fn confirm(&mut self, _: &PickerItem, window: &mut Window, cx: &mut App) {
        // Enter hands the focus to the explorer, where the table is already selected.
        let ws = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            ws.update(cx, |_, cx| {
                window.dispatch_action(Box::new(crate::actions::FocusExplorer), cx);
            })
            .ok();
        });
    }
}
