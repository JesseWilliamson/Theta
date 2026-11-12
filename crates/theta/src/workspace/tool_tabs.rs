//! Diagram and comparison tabs: opening them, the events they emit, and their entry points (palette,
//! chord, `:` command, explorer menu). The views live in `crate::er` and the comparison modules; like the
//! table editor and the admin tabs they never touch the workspace, they emit events that are handled here.

use super::{ConsoleTab, ToolTab, Workspace};
use crate::connections::NoticeKind;
use crate::data_compare::{DataCompareView, Ends};
use crate::dialogs::compare_pick;
use crate::er::{DiagramEvent, DiagramView};
use crate::palette::ChoicePicker;
use crate::schema_compare::{CompareEvent, Endpoint, SchemaCompareView};
use gpui_kit::assets::IconName;
use gpui_kit::{AppContext as _, Context, Window};
use theta_diagram::Scope;
use theta_ui::picker::PickerItem;

impl ToolTab {
    pub fn focus(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        match self {
            ToolTab::Diagram(v) => {
                let h = v.read(cx).focus_handle_ref();
                window.focus(&h, cx);
            }
            ToolTab::SchemaCompare(v) => {
                let h = v.read(cx).focus_handle_ref();
                window.focus(&h, cx);
            }
            ToolTab::DataCompare(v) => {
                let h = v.read(cx).focus_handle_ref();
                window.focus(&h, cx);
            }
        }
    }

    pub fn conn_id(&self, cx: &gpui_kit::App) -> String {
        match self {
            ToolTab::Diagram(v) => v.read(cx).conn_id().to_string(),
            ToolTab::SchemaCompare(v) => v.read(cx).target_conn().to_string(),
            ToolTab::DataCompare(v) => v.read(cx).target_conn().to_string(),
        }
    }
}

/// Schemas that hold the database's own tables; offered last, never as a default.
pub(crate) fn is_system_schema(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "information_schema" | "pg_catalog" | "pg_toast" | "performance_schema" | "mysql" | "sys"
    )
}

impl Workspace {
    pub fn tool_tab(&self, tab: u64) -> Option<ToolTab> {
        self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.tool.clone())
    }

    /// Open an ER diagram (or switch to the one already open for the same scope).
    pub fn open_diagram(
        &mut self,
        conn_id: String,
        scope: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let Some(catalog) = self.store.read(cx).catalog(&conn_id) else {
            self.notify(
                NoticeKind::Info,
                "Connect and wait for the schema to load, then open the diagram",
                window,
                cx,
            );
            return None;
        };
        let existing = self.tabs.iter().find_map(|t| match &t.tool {
            Some(ToolTab::Diagram(v)) => {
                let v = v.read(cx);
                (v.conn_id() == conn_id && *v.scope() == scope).then_some((t.id, v_entity(&t.tool)))
            }
            _ => None,
        });
        if let Some((id, Some(v))) = existing {
            v.update(cx, |v, cx| v.set_catalog(&catalog, cx));
            self.activate_tab(id, window, cx);
            return Some(id);
        }
        let conn = conn_id.clone();
        let view = cx.new(|cx| DiagramView::new(conn, scope, &catalog, window, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let sub = cx.subscribe_in(&view, window, move |this, v, ev: &DiagramEvent, window, cx| {
            this.on_diagram_event(id, v.clone(), ev.clone(), window, cx)
        });
        self.table_subs.insert(id, sub);
        let title = view.read(cx).title();
        self.tabs.push(ConsoleTab {
            id,
            title: title.into(),
            connection_id: Some(conn_id.clone()),
            modified: false,
            initial_sql: None,
            table: None,
            admin: None,
            tool: Some(ToolTab::Diagram(view.clone())),
        });
        self.console_views.insert(id, view.clone().into());
        self.active_connection = Some(conn_id);
        self.activate_tab(id, window, cx);
        // A picker that opened this tab restores the focus it saved when it closes, which can land after
        // the activation above; focus once more after that.
        window.defer(cx, move |window, cx| {
            let h = view.read(cx).focus_handle_ref();
            window.focus(&h, cx);
        });
        Some(id)
    }

    /// Ask what to compare (a connection and a schema for each side), then open the result tab.
    /// `preset` is a schema to start the source on (the explorer's context menu).
    pub(super) fn compare_schemas_dialog(
        &mut self,
        preset: Option<(String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quiet {
            return;
        }
        let choices = compare_pick::choices(&self.store, cx);
        if choices.is_empty() {
            self.notify(
                NoticeKind::Info,
                "Connect to a database and wait for its schema to load first",
                window,
                cx,
            );
            return;
        }
        let active = self.active_connection_id(cx);
        let start = preset
            .map(|(conn_id, schema)| compare_pick::Side { conn_id, schema, table: None })
            .or_else(|| {
                active.and_then(|id| {
                    let cat = self.store.read(cx).catalog(&id)?;
                    Some(compare_pick::Side {
                        conn_id: id,
                        schema: cat.default_schema.clone().unwrap_or_default(),
                        table: None,
                    })
                })
            });
        let model = compare_pick::Model::new(compare_pick::Mode::Schema, choices, start.as_ref());
        let ws = cx.entity().downgrade();
        compare_pick::open(
            "Compare Schemas",
            "Compare",
            model,
            std::rc::Rc::new(move |picked, window, cx| {
                let _ = ws.update(cx, |ws, cx| ws.open_schema_compare(picked, window, cx));
            }),
            window,
            cx,
        );
    }

    fn endpoint(&self, side: &compare_pick::Side, cx: &gpui_kit::App) -> Option<Endpoint> {
        let store = self.store.read(cx);
        let cat = store.catalog(&side.conn_id)?;
        let config = store.config(&side.conn_id)?;
        Some(Endpoint {
            conn_id: side.conn_id.clone(),
            conn_name: config.name.clone(),
            kind: config.kind,
            schema: cat.schema(&side.schema)?.clone(),
        })
    }

    pub fn open_schema_compare(
        &mut self,
        picked: compare_pick::Picked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(source), Some(target)) =
            (self.endpoint(&picked.source, cx), self.endpoint(&picked.target, cx))
        else {
            self.notify(NoticeKind::Warning, "A schema to compare is no longer loaded", window, cx);
            return;
        };
        let title = crate::schema_compare::title_of(&source, &target);
        let view = cx.new(|cx| SchemaCompareView::new(source, target, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let sub = cx.subscribe_in(&view, window, move |this, v, ev: &CompareEvent, window, cx| {
            let sides = v.read(cx).sides();
            this.on_compare_event(v.clone(), sides, ev.clone(), window, cx)
        });
        self.table_subs.insert(id, sub);
        let target_conn = view.read(cx).target_conn().to_string();
        self.tabs.push(ConsoleTab {
            id,
            title: title.into(),
            connection_id: Some(target_conn.clone()),
            modified: false,
            initial_sql: None,
            table: None,
            admin: None,
            tool: Some(ToolTab::SchemaCompare(view.clone())),
        });
        self.console_views.insert(id, view.clone().into());
        self.active_connection = Some(target_conn);
        self.activate_tab(id, window, cx);
        window.defer(cx, move |window, cx| {
            let h = view.read(cx).focus_handle_ref();
            window.focus(&h, cx);
        });
    }

    fn on_compare_event(
        &mut self,
        view: gpui_kit::Entity<SchemaCompareView>,
        sides: (compare_pick::Side, compare_pick::Side),
        ev: CompareEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            CompareEvent::OpenScript { conn_id, sql } => {
                let id = self.open_console(Some(conn_id), Some(sql), window, cx);
                self.set_tab_title(id, "Migration script".to_string(), cx);
            }
            CompareEvent::Notice(kind, m) => self.notify(kind, m, window, cx),
            CompareEvent::Reload => {
                let (Some(source), Some(target)) = (self.endpoint(&sides.0, cx), self.endpoint(&sides.1, cx))
                else {
                    self.notify(NoticeKind::Warning, "A schema to compare is no longer loaded", window, cx);
                    return;
                };
                view.update(cx, |v, cx| v.recompare(source, target, cx));
                self.notify(NoticeKind::Info, "Compared again with the schemas as loaded now", window, cx);
            }
        }
    }

    /// Ask which two tables to compare and by which key, then open the result tab, which starts reading.
    pub(super) fn compare_data_dialog(
        &mut self,
        preset: Option<(String, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quiet {
            return;
        }
        let choices = compare_pick::choices(&self.store, cx);
        if choices.is_empty() {
            self.notify(
                NoticeKind::Info,
                "Connect to a database and wait for its schema to load first",
                window,
                cx,
            );
            return;
        }
        let start = preset
            .map(|(conn_id, schema, table)| compare_pick::Side { conn_id, schema, table: Some(table) })
            .or_else(|| {
                let id = self.active_connection_id(cx)?;
                let cat = self.store.read(cx).catalog(&id)?;
                Some(compare_pick::Side {
                    conn_id: id,
                    schema: cat.default_schema.clone().unwrap_or_default(),
                    table: None,
                })
            });
        let model = compare_pick::Model::new(compare_pick::Mode::Table, choices, start.as_ref());
        let ws = cx.entity().downgrade();
        compare_pick::open(
            "Compare Table Data",
            "Compare",
            model,
            std::rc::Rc::new(move |picked, window, cx| {
                let _ = ws.update(cx, |ws, cx| ws.open_data_compare(picked, window, cx));
            }),
            window,
            cx,
        );
    }

    pub fn open_data_compare(
        &mut self,
        picked: compare_pick::Picked,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let table_of = |ep: &Endpoint, name: &Option<String>| {
            name.as_ref()
                .and_then(|n| ep.schema.tables.iter().find(|t| t.name.eq_ignore_ascii_case(n)).cloned())
        };
        let (Some(source), Some(target)) =
            (self.endpoint(&picked.source, cx), self.endpoint(&picked.target, cx))
        else {
            self.notify(NoticeKind::Warning, "A table to compare is no longer loaded", window, cx);
            return;
        };
        let (Some(st), Some(tt)) =
            (table_of(&source, &picked.source.table), table_of(&target, &picked.target.table))
        else {
            self.notify(NoticeKind::Warning, "A table to compare is no longer loaded", window, cx);
            return;
        };
        for e in [&source, &target] {
            let why =
                self.store.read(cx).config(&e.conn_id).and_then(crate::data_compare::side_session_blocker);
            if let Some(why) = why {
                self.notify(NoticeKind::Warning, why, window, cx);
                return;
            }
        }
        let mapping = match theta_sql::data_diff::map_columns(&st, &tt, &picked.key) {
            Ok(m) => m,
            Err(why) => {
                self.notify(NoticeKind::Warning, why, window, cx);
                return;
            }
        };
        let ends = Ends { source, source_table: st, target, target_table: tt, key: picked.key };
        let title = crate::data_compare::title_of(&ends);
        let target_conn = ends.target.conn_id.clone();
        let store = self.store.clone();
        let view = cx.new(|cx| DataCompareView::new(store, ends, mapping, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let sub =
            cx.subscribe_in(&view, window, move |this, v, ev: &CompareEvent, window, cx| match ev.clone() {
                CompareEvent::OpenScript { conn_id, sql } => {
                    let id = this.open_console(Some(conn_id), Some(sql), window, cx);
                    this.set_tab_title(id, "Sync script".to_string(), cx);
                }
                CompareEvent::Notice(kind, m) => this.notify(kind, m, window, cx),
                CompareEvent::Reload => v.update(cx, |v, cx| v.start(cx)),
            });
        self.table_subs.insert(id, sub);
        self.tabs.push(ConsoleTab {
            id,
            title: title.into(),
            connection_id: Some(target_conn.clone()),
            modified: false,
            initial_sql: None,
            table: None,
            admin: None,
            tool: Some(ToolTab::DataCompare(view.clone())),
        });
        self.console_views.insert(id, view.clone().into());
        self.active_connection = Some(target_conn);
        self.activate_tab(id, window, cx);
        window.defer(cx, move |window, cx| {
            let h = view.read(cx).focus_handle_ref();
            window.focus(&h, cx);
        });
    }

    fn on_diagram_event(
        &mut self,
        tab: u64,
        view: gpui_kit::Entity<DiagramView>,
        ev: DiagramEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            DiagramEvent::OpenTable { schema, table } => {
                let conn = view.read(cx).conn_id().to_string();
                self.open_table(conn, schema, table, window, cx);
            }
            DiagramEvent::SaveFile { suggested, text, done } => {
                self.save_text_file(suggested, text, done, window, cx)
            }
            DiagramEvent::Notice(kind, message) => self.notify(kind, message, window, cx),
            DiagramEvent::Reload => {
                let conn = view.read(cx).conn_id().to_string();
                match self.store.read(cx).catalog(&conn) {
                    Some(c) => {
                        view.update(cx, |v, cx| v.set_catalog(&c, cx));
                        self.notify(NoticeKind::Info, "Diagram rebuilt from the loaded schema", window, cx);
                    }
                    None => self.notify(NoticeKind::Info, "The schema is not loaded", window, cx),
                }
                let _ = tab;
            }
        }
    }

    /// The palette, chord and `:diagram [schema | table]` entry. With an argument it opens that schema or
    /// table at once; without, it asks.
    pub(super) fn diagram_from_palette(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conn) = self.active_connection_id(cx) else {
            self.notify(NoticeKind::Info, "Add a connection first", window, cx);
            return;
        };
        let Some(catalog) = self.store.read(cx).catalog(&conn) else {
            self.notify(NoticeKind::Info, "Connect to a database to draw its diagram", window, cx);
            return;
        };
        let arg = arg.trim();
        if !arg.is_empty() {
            let scope = if let Some(s) = catalog.schema(arg) {
                Some(Scope::Schema(s.name.clone()))
            } else {
                let (schema, table) = match arg.split_once('.') {
                    Some((s, t)) => (Some(s), t),
                    None => (None, arg),
                };
                catalog
                    .find_table(schema, table)
                    .map(|(s, t)| Scope::Table { schema: s.name.clone(), table: t.name.clone() })
            };
            match scope {
                Some(scope) => {
                    self.open_diagram(conn, scope, window, cx);
                }
                None => {
                    self.notify(NoticeKind::Error, format!("No schema or table named “{arg}”"), window, cx)
                }
            }
            return;
        }
        let mut items: Vec<PickerItem> = Vec::new();
        let mut schemas: Vec<&theta_core::Schema> =
            catalog.schemas.iter().filter(|s| !s.tables.is_empty()).collect();
        schemas.sort_by_key(|s| (is_system_schema(&s.name), s.name.to_lowercase()));
        for s in &schemas {
            // The diagram draws tables only, so the count says tables and names the views left out; the
            // picker and the diagram's own status line then agree.
            let tables = s.tables.iter().filter(|t| t.kind == theta_core::TableKind::Table).count() as u64;
            let views = s.tables.len() as u64 - tables;
            let detail = if views == 0 {
                theta_core::text::plural(tables, "table", "tables")
            } else {
                format!(
                    "{} ({} not drawn)",
                    theta_core::text::plural(tables, "table", "tables"),
                    theta_core::text::plural(views, "view", "views")
                )
            };
            items.push(
                PickerItem::new(format!("s\u{1}{}", s.name), format!("Schema {}", s.name))
                    .icon(IconName::Network)
                    .detail(detail)
                    .section("Whole schema"),
            );
        }
        for s in &schemas {
            for t in &s.tables {
                items.push(
                    PickerItem::new(
                        format!("t\u{1}{}\u{1}{}", s.name, t.name),
                        format!("{}.{}", s.name, t.name),
                    )
                    .icon(IconName::Table2)
                    .section("A table and its neighbours"),
                );
            }
        }
        if items.is_empty() {
            self.notify(NoticeKind::Info, "The loaded schema has no tables to draw", window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let picker = ChoicePicker::new(
            "Draw a schema, or a table and its neighbours…",
            items,
            move |id, window, cx| {
                let mut parts = id.split('\u{1}');
                let scope = match (parts.next(), parts.next(), parts.next()) {
                    (Some("s"), Some(s), _) => Scope::Schema(s.to_string()),
                    (Some("t"), Some(s), Some(t)) => {
                        Scope::Table { schema: s.to_string(), table: t.to_string() }
                    }
                    _ => return,
                };
                let conn = conn.clone();
                let _ = ws.update(cx, |ws, cx| {
                    ws.open_diagram(conn, scope, window, cx);
                });
            },
        );
        self.open_picker(picker, window, cx);
    }
}

fn v_entity(tool: &Option<ToolTab>) -> Option<gpui_kit::Entity<DiagramView>> {
    match tool {
        Some(ToolTab::Diagram(v)) => Some(v.clone()),
        _ => None,
    }
}
