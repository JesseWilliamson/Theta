//! Live templates in the workspace: expanding an abbreviation (which may need a table picked first),
//! "Surround with", and opening the templates file.

use super::Workspace;
use crate::connections::NoticeKind;
use gpui_kit::{App, Context, SharedString, Window};
use std::ops::Range;
use theta_sql::templates::{Vars, needs_table, resolve_variables, selection_value, surround_options};
use theta_ui::picker::{PickerDelegate, PickerItem};

impl Workspace {
    fn connection_of(&self, console: u64) -> Option<String> {
        self.tabs.iter().find(|t| t.id == console).and_then(|t| t.connection_id.clone())
    }

    fn base_vars(&self, console: u64, selection: &str, cx: &App) -> Vars {
        let _ = console;
        Vars {
            selection: selection_value(selection),
            clipboard: cx.read_from_clipboard().and_then(|i| i.text()).unwrap_or_default(),
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            ..Vars::default()
        }
    }

    /// An abbreviation was accepted (Tab, or a completion item): substitute the variables and insert it.
    pub(super) fn expand_template(
        &mut self,
        console: u64,
        body: String,
        replace: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !needs_table(&body) {
            let vars = self.base_vars(console, "", cx);
            self.insert_template(console, &resolve_variables(&body, &vars), replace, window, cx);
            return;
        }
        let connected = self.connection_of(console).and_then(|c| self.store.read(cx).catalog(&c)).is_some();
        if !connected {
            self.notify(
                NoticeKind::Info,
                "This template needs a table: connect to a database first",
                window,
                cx,
            );
            return;
        }
        let delegate = TemplateTablePicker {
            workspace: cx.entity().downgrade(),
            console,
            body,
            replace,
            tables: self.table_choices(console, cx),
        };
        self.open_picker(delegate, window, cx);
    }

    /// (schema, table) of every table in the console's catalog.
    fn table_choices(&self, console: u64, cx: &App) -> Vec<(String, String)> {
        let Some(catalog) = self.connection_of(console).and_then(|c| self.store.read(cx).catalog(&c)) else {
            return Vec::new();
        };
        catalog
            .schemas
            .iter()
            .flat_map(|s| s.tables.iter().map(move |t| (s.name.clone(), t.name.clone())))
            .collect()
    }

    /// The table was chosen: fill `${table}` and `${columns}` from the catalog and insert.
    fn finish_template(
        &mut self,
        console: u64,
        body: String,
        replace: Range<usize>,
        table: (String, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.connection_of(console) else { return };
        let (catalog, kind) = {
            let store = self.store.read(cx);
            (store.catalog(&conn), store.config(&conn).map(|c| c.kind))
        };
        let (Some(catalog), Some(kind)) = (catalog, kind) else { return };
        let mut vars = self.base_vars(console, "", cx);
        let default = catalog.default_schema.as_deref();
        let schema = (default != Some(table.0.as_str())).then_some(table.0.as_str());
        vars.table = Some(theta_sql::keywords::qualified_name(schema, &table.1, kind));
        if let Some((_, t)) = catalog.find_table(Some(&table.0), &table.1) {
            vars.columns =
                t.columns.iter().map(|c| theta_sql::keywords::quote_if_needed(&c.name, kind)).collect();
        }
        self.insert_template(console, &resolve_variables(&body, &vars), replace, window, cx);
    }

    fn insert_template(
        &mut self,
        console: u64,
        resolved: &str,
        replace: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.consoles.get(&console) else { return };
        let editor = c.editor.clone();
        editor.update(cx, |e, cx| e.insert_snippet(replace, resolved, cx));
        // The picker (if any) restored focus to wherever it came from; typing continues in the editor.
        if self.active_tab == Some(console) {
            self.focus_editor(window, cx);
        }
    }

    /// *Surround with…*: pick a wrapper for the selected text.
    pub(super) fn open_surround_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab else {
            self.notify(NoticeKind::Info, "Open a console first (Ctrl+N)", window, cx);
            return;
        };
        let selected = self.consoles.get(&id).and_then(|c| c.editor.read(cx).editor().selected_text());
        if selected.as_deref().is_none_or(|s| s.trim().is_empty()) {
            self.notify(NoticeKind::Info, "Select some SQL to surround first", window, cx);
            return;
        }
        let delegate = SurroundPicker { workspace: cx.entity().downgrade(), console: id };
        self.open_picker(delegate, window, cx);
    }

    fn surround(&mut self, console: u64, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.consoles.get(&console) else { return };
        let (range, text) = {
            let ed = c.editor.read(cx).editor();
            let p = ed.primary();
            (p.from()..p.to(), ed.selected_text().unwrap_or_default())
        };
        let vars = self.base_vars(console, &text, cx);
        self.insert_template(console, &resolve_variables(body, &vars), range, window, cx);
    }

    /// *Edit Live Templates…*: create the file if needed and open it in the system editor.
    pub(super) fn edit_templates_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = crate::templates::TemplateStore::global();
        if let Err(e) = store.ensure_file() {
            self.notify(
                NoticeKind::Error,
                format!(
                    "Could not create {}. {}",
                    store.path().display(),
                    theta_core::friendly::plain(&e.to_string())
                ),
                window,
                cx,
            );
            return;
        }
        // The platform gives no answer on whether an application took the file, so the message names the
        // file and what to do when nothing opened.
        cx.open_with_system(store.path());
        self.notify(
            NoticeKind::Info,
            format!(
                "Asked the system to open {}. If nothing opens, no application is set for .json files: open the file by hand.",
                store.path().display()
            ),
            window,
            cx,
        );
    }
}

struct TemplateTablePicker {
    workspace: gpui_kit::WeakEntity<Workspace>,
    console: u64,
    body: String,
    replace: Range<usize>,
    tables: Vec<(String, String)>,
}

impl PickerDelegate for TemplateTablePicker {
    fn placeholder(&self) -> SharedString {
        "Table for the template…".into()
    }
    fn noun(&self) -> SharedString {
        "tables".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        self.tables
            .iter()
            .enumerate()
            .map(|(i, (s, t))| {
                PickerItem::new(i.to_string(), format!("{s}.{t}")).icon(gpui_kit::assets::IconName::Table2)
            })
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some(table) = item.id.parse::<usize>().ok().and_then(|i| self.tables.get(i)).cloned() else {
            return;
        };
        let (ws, console, body, replace) =
            (self.workspace.clone(), self.console, self.body.clone(), self.replace.clone());
        // After the picker has closed and given focus back.
        window.defer(cx, move |window, cx| {
            ws.update(cx, |this, cx| this.finish_template(console, body, replace, table, window, cx)).ok();
        });
    }
}

struct SurroundPicker {
    workspace: gpui_kit::WeakEntity<Workspace>,
    console: u64,
}

impl PickerDelegate for SurroundPicker {
    fn placeholder(&self) -> SharedString {
        "Surround the selection with…".into()
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        surround_options()
            .iter()
            .enumerate()
            .map(|(i, s)| PickerItem::new(i.to_string(), s.label).icon(gpui_kit::assets::IconName::Scissors))
            .collect()
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let Some(body) =
            item.id.parse::<usize>().ok().and_then(|i| surround_options().get(i).map(|s| s.body))
        else {
            return;
        };
        let (ws, console) = (self.workspace.clone(), self.console);
        window.defer(cx, move |window, cx| {
            ws.update(cx, |this, cx| this.surround(console, body, window, cx)).ok();
        });
    }
}
