//! The Schema Compare result tab: the differences as a tree on the left, the chosen difference side by side on
//! the right, and the migration script one key away.
//!
//! The comparison and the script are `theta_sql::schema_diff` (pure, with live round-trip tests); this view
//! only holds the two schemas it compared, flattens the diff into rows, and paints. It never runs anything:
//! *Open Migration Script in Console* hands the text to a console bound to the **target** connection, and that
//! is as far as it goes.

use crate::connections::NoticeKind;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollStrategy,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, div, px, uniform_list,
};
use std::collections::HashSet;
use std::ops::Range;
use theta_core::{DatabaseKind, Schema};
use theta_sql::schema_diff::{
    self, Change, NOT_COMPARED, ObjectDiff, ObjectKind, SchemaDiff, Script, Side, Status,
};

/// One end of the comparison.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub conn_id: String,
    pub conn_name: String,
    pub kind: DatabaseKind,
    pub schema: Schema,
}

impl Endpoint {
    fn label(&self) -> String {
        format!("{} · {}", self.conn_name, self.schema.name)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompareEvent {
    /// Open a console on the target connection with this text. Nothing runs.
    OpenScript {
        conn_id: String,
        sql: String,
    },
    Notice(NoticeKind, String),
    /// Compare again with the catalogs as they are now.
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Object(usize),
    Change(usize, usize),
}

pub struct SchemaCompareView {
    source: Endpoint,
    target: Endpoint,
    diff: SchemaDiff,
    expanded: HashSet<usize>,
    rows: Vec<Row>,
    sel: usize,
    include_destructive: bool,
    script: Result<Script, String>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl EventEmitter<CompareEvent> for SchemaCompareView {}

impl Focusable for SchemaCompareView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub fn title_of(source: &Endpoint, target: &Endpoint) -> String {
    format!("Compare: {} → {}", source.schema.name, target.schema.name)
}

impl SchemaCompareView {
    pub fn new(source: Endpoint, target: Endpoint, cx: &mut Context<Self>) -> Self {
        let diff = schema_diff::compare(
            Side { kind: source.kind, schema: &source.schema },
            Side { kind: target.kind, schema: &target.schema },
        );
        let mut v = SchemaCompareView {
            script: Err(String::new()),
            source,
            target,
            diff,
            expanded: HashSet::new(),
            rows: Vec::new(),
            sel: 0,
            include_destructive: false,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        // Tables with a few differences start open: that is what you came to read.
        for (i, o) in v.diff.objects.iter().enumerate() {
            if o.status == Status::Different && v.diff.objects.len() <= 12 {
                v.expanded.insert(i);
            }
        }
        v.rebuild_rows();
        v.regenerate();
        v
    }

    pub fn title(&self) -> String {
        title_of(&self.source, &self.target)
    }

    pub fn conn_ids(&self) -> (&str, &str) {
        (&self.source.conn_id, &self.target.conn_id)
    }

    /// The two sides in the form the pick dialog uses, to compare again.
    pub fn sides(&self) -> (crate::dialogs::compare_pick::Side, crate::dialogs::compare_pick::Side) {
        let side = |e: &Endpoint| crate::dialogs::compare_pick::Side {
            conn_id: e.conn_id.clone(),
            schema: e.schema.name.clone(),
            table: None,
        };
        (side(&self.source), side(&self.target))
    }

    /// Replace the schemas with fresh ones and compare again, keeping the cursor on the same object if it
    /// is still there.
    pub fn recompare(&mut self, source: Endpoint, target: Endpoint, cx: &mut Context<Self>) {
        let at = self.rows.get(self.sel).map(|r| match r {
            Row::Object(i) | Row::Change(i, _) => self.diff.objects[*i].name.clone(),
        });
        self.source = source;
        self.target = target;
        self.diff = schema_diff::compare(
            Side { kind: self.source.kind, schema: &self.source.schema },
            Side { kind: self.target.kind, schema: &self.target.schema },
        );
        self.expanded.clear();
        if self.diff.objects.len() <= 12 {
            self.expanded = (0..self.diff.objects.len()).collect();
        }
        self.rebuild_rows();
        if let Some(n) = at
            && let Some(p) =
                self.rows.iter().position(|r| matches!(r, Row::Object(i) if self.diff.objects[*i].name == n))
        {
            self.sel = p;
        }
        self.regenerate();
        cx.notify();
    }

    pub fn target_conn(&self) -> &str {
        &self.target.conn_id
    }

    pub fn diff(&self) -> &SchemaDiff {
        &self.diff
    }

    pub fn script(&self) -> &Result<Script, String> {
        &self.script
    }

    pub fn focus_handle_ref(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (i, o) in self.diff.objects.iter().enumerate() {
            self.rows.push(Row::Object(i));
            if self.expanded.contains(&i) {
                for j in 0..o.changes.len() {
                    self.rows.push(Row::Change(i, j));
                }
            }
        }
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
    }

    fn regenerate(&mut self) {
        self.script = schema_diff::generate(
            Side { kind: self.source.kind, schema: &self.source.schema },
            Side { kind: self.target.kind, schema: &self.target.schema },
            &self.diff,
            self.include_destructive,
        );
    }

    pub fn script_text(&self) -> Option<String> {
        self.script
            .as_ref()
            .ok()
            .map(|s| s.render(self.target.kind, &self.source.label(), &self.target.label()))
    }

    fn move_by(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.sel = (self.sel as isize + by).clamp(0, self.rows.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(self.sel, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Open or close the object under the cursor (from a change row: its object).
    fn toggle(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.sel).copied() else { return };
        let (Row::Object(i) | Row::Change(i, _)) = row;
        if self.diff.objects[i].changes.is_empty() {
            return;
        }
        let now = self.expanded.contains(&i);
        match open.unwrap_or(!now) {
            true => self.expanded.insert(i),
            false => self.expanded.remove(&i),
        };
        self.rebuild_rows();
        // Closing from a change row leaves the cursor on the object.
        if let Some(p) = self.rows.iter().position(|r| *r == Row::Object(i))
            && matches!(row, Row::Change(..))
            && open == Some(false)
        {
            self.sel = p;
        }
        cx.notify();
    }

    pub(crate) fn toggle_destructive(&mut self, cx: &mut Context<Self>) {
        self.include_destructive = !self.include_destructive;
        self.regenerate();
        cx.notify();
    }

    pub(crate) fn open_script(&mut self, cx: &mut Context<Self>) {
        match (&self.script, self.script_text()) {
            (Ok(s), Some(text)) if !s.is_empty() || !s.manual.is_empty() => {
                cx.emit(CompareEvent::OpenScript { conn_id: self.target.conn_id.clone(), sql: text })
            }
            (Ok(_), _) => cx.emit(CompareEvent::Notice(
                NoticeKind::Info,
                "There is nothing to migrate: the schemas are identical in everything that was compared"
                    .into(),
            )),
            (Err(why), _) => cx.emit(CompareEvent::Notice(NoticeKind::Warning, why.clone())),
        }
    }

    pub(crate) fn copy_script(&mut self, cx: &mut Context<Self>) {
        match self.script_text() {
            Some(t) => {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
                cx.emit(CompareEvent::Notice(NoticeKind::Success, "Copied the migration script".into()));
            }
            None => {
                let why = self.script.as_ref().err().cloned().unwrap_or_default();
                cx.emit(CompareEvent::Notice(NoticeKind::Warning, why));
            }
        }
    }

    /// Read both schemas again (the `r` key).
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        cx.emit(CompareEvent::Reload);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return;
        }
        match ev.keystroke.key.as_str() {
            "down" | "j" => self.move_by(1, cx),
            "up" | "k" => self.move_by(-1, cx),
            "pagedown" => self.move_by(10, cx),
            "pageup" => self.move_by(-10, cx),
            "home" | "g" if !m.shift => {
                self.sel = 0;
                self.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            "end" | "g" => {
                self.sel = self.rows.len().saturating_sub(1);
                self.scroll.scroll_to_item(self.sel, ScrollStrategy::Bottom);
                cx.notify();
            }
            "right" | "l" => self.toggle(Some(true), cx),
            "left" | "h" => self.toggle(Some(false), cx),
            "space" => self.toggle(None, cx),
            "d" => self.toggle_destructive(cx),
            "enter" | "o" => self.open_script(cx),
            "c" => self.copy_script(cx),
            "r" => cx.emit(CompareEvent::Reload),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn object(&self, i: usize) -> &ObjectDiff {
        &self.diff.objects[i]
    }
}

fn mark(s: Status) -> &'static str {
    match s {
        Status::OnlyInSource => "+",
        Status::OnlyInTarget => "−",
        Status::Different => "~",
    }
}

fn plural(n: usize, one: &str) -> String {
    theta_core::text::plural_s(n as u64, one)
}

impl Render for SchemaCompareView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus.is_focused(window);
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let colour = |s: Status| match s {
            Status::OnlyInSource => theme.green,
            Status::OnlyInTarget => theme.red,
            Status::Different => theme.yellow,
        };

        // The script line: what it is, or why there is none.
        let script_line: AnyElement = match &self.script {
            Ok(s) => {
                let held = s.commented_count();
                let mut parts = vec![plural(s.live_count(), "statement")];
                if held > 0 {
                    parts.push(format!("{held} destructive held back (d includes them)"));
                } else if self.include_destructive {
                    parts.push("destructive statements included".to_string());
                }
                if !s.manual.is_empty() {
                    parts.push(format!("{} to do by hand", s.manual.len()));
                }
                div().child(format!("Script: {}", parts.join(" · "))).into_any_element()
            }
            Err(why) => div().text_color(theme.warning).child(format!("No script: {why}")).into_any_element(),
        };
        let header = v_flex()
            .px_3()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(self.source.label()))
                    .child(div().text_color(theme.muted_foreground).child("→ make"))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(self.target.label()))
                    .child(div().text_color(theme.muted_foreground).child("match it")),
            )
            .child(div().text_size(px(12.)).child(self.diff.summary()))
            .child(
                h_flex()
                    .gap_4()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(script_line),
            );

        let sel_row = self.rows.get(self.sel).copied();
        let list = uniform_list(
            "schema-compare-rows",
            self.rows.len(),
            cx.processor(move |this, range: Range<usize>, _w, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|i| {
                        let row = this.rows[i];
                        let selected = i == this.sel;
                        let (indent, status, label, tag, open) = match row {
                            Row::Object(o) => {
                                let ob = this.object(o);
                                (
                                    0.,
                                    ob.status,
                                    ob.name.clone(),
                                    ob.kind.label().to_string(),
                                    (!ob.changes.is_empty()).then(|| this.expanded.contains(&o)),
                                )
                            }
                            Row::Change(o, c) => {
                                let ch = &this.object(o).changes[c];
                                let attrs = if ch.attributes.is_empty() {
                                    String::new()
                                } else {
                                    format!(" ({})", ch.attributes.join(", "))
                                };
                                (
                                    22.,
                                    ch.status,
                                    format!("{} {}{attrs}", ch.element.label(), ch.name),
                                    String::new(),
                                    None,
                                )
                            }
                        };
                        let c = match status {
                            Status::OnlyInSource => theme.green,
                            Status::OnlyInTarget => theme.red,
                            Status::Different => theme.yellow,
                        };
                        h_flex()
                            .id(("compare-row", i))
                            .h(px(24.))
                            .w_full()
                            .flex_none()
                            .px_2()
                            .pl(px(8. + indent))
                            .gap_2()
                            .items_center()
                            .text_size(px(12.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(theme.list_active))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.focus(&this.focus, cx);
                                this.sel = i;
                                if matches!(row, Row::Object(_)) {
                                    this.toggle(None, cx);
                                }
                                cx.notify();
                            }))
                            .child(div().w(px(10.)).text_color(theme.muted_foreground).child(match open {
                                Some(true) => "▾",
                                Some(false) => "▸",
                                None => "",
                            }))
                            .child(
                                div()
                                    .w(px(12.))
                                    .text_color(c)
                                    .font_weight(FontWeight::BOLD)
                                    .child(mark(status)),
                            )
                            .child(div().flex_1().truncate().child(label))
                            .child(div().text_color(theme.muted_foreground).text_size(px(11.)).child(tag))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .w_full()
        .flex_1()
        .min_h_0();

        let tree: AnyElement = if self.rows.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child("No differences in anything that was compared")
                .into_any_element()
        } else {
            list.into_any_element()
        };

        let detail: AnyElement = match sel_row {
            None => div().into_any_element(),
            Some(Row::Object(i)) => {
                let o = self.object(i);
                self.detail_object(o, &mono, &theme, colour(o.status))
            }
            Some(Row::Change(i, j)) => {
                let o = self.object(i);
                self.detail_change(o, &o.changes[j], &mono, &theme, colour(o.changes[j].status))
            }
        };

        let footer = v_flex()
            .px_3()
            .py_1()
            .gap_1()
            .border_t_2()
            // The only focus mark the tab needs: without it nothing says the keys reach it.
            .border_color(if focused { theme.primary } else { theme.border })
            .text_size(px(11.))
            .text_color(theme.muted_foreground)
            .child(format!("Not compared (the catalog does not carry it): {}.", NOT_COMPARED.join(", ")))
            .child("j k move · l h open and close (space toggles) · d destructive on / off · enter or o open the script in a console · c copy it · r compare again · + only in source, − only in target, ~ different");

        v_flex()
            .size_full()
            .bg(theme.background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(v_flex().w(px(420.)).h_full().border_r_1().border_color(theme.border).child(tree))
                    .child(div().flex_1().h_full().min_w_0().overflow_hidden().child(detail)),
            )
            .child(footer)
    }
}

impl SchemaCompareView {
    fn side_by_side(
        &self,
        source: Option<String>,
        target: Option<String>,
        mono: &str,
        theme: &gpui_kit::component::Theme,
    ) -> AnyElement {
        let pane = |title: String, text: Option<String>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(title))
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .font_family(mono.to_string())
                        .text_size(px(12.))
                        .child(text.unwrap_or_else(|| "(does not exist)".to_string())),
                )
        };
        h_flex()
            .gap_3()
            .items_start()
            .child(pane(format!("Source · {}", self.source.label()), source))
            .child(pane(format!("Target · {}", self.target.label()), target))
            .into_any_element()
    }

    fn detail_change(
        &self,
        o: &ObjectDiff,
        c: &Change,
        mono: &str,
        theme: &gpui_kit::component::Theme,
        colour: gpui_kit::Hsla,
    ) -> AnyElement {
        let what = match c.status {
            Status::OnlyInSource => "exists only in the source: the script adds it",
            Status::OnlyInTarget => "exists only in the target: the script drops it (destructive)",
            Status::Different => "differs: the script changes the target to match the source",
        };
        v_flex()
            .id("compare-detail")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                "{} · {} {}",
                o.name,
                c.element.label(),
                c.name
            )))
            .child(div().text_color(colour).text_size(px(12.)).child(what))
            .when(!c.attributes.is_empty(), |d| {
                d.child(div().text_size(px(12.)).child(format!("What differs: {}", c.attributes.join(", "))))
            })
            .child(self.side_by_side(c.source.clone(), c.target.clone(), mono, theme))
            .into_any_element()
    }

    fn detail_object(
        &self,
        o: &ObjectDiff,
        mono: &str,
        theme: &gpui_kit::component::Theme,
        colour: gpui_kit::Hsla,
    ) -> AnyElement {
        let what = match (o.kind, o.status) {
            (ObjectKind::Routine, Status::OnlyInSource) => {
                "exists only in the source (the catalog has no routine body, so no script can create it)"
                    .to_string()
            }
            (ObjectKind::Routine, Status::OnlyInTarget) => {
                "exists only in the target (not dropped by the script)".to_string()
            }
            (_, Status::OnlyInSource) => {
                format!("The {} exists only in the source: the script creates it.", o.kind.label())
            }
            (_, Status::OnlyInTarget) => format!(
                "The {} exists only in the target: the script drops it (destructive).",
                o.kind.label()
            ),
            (_, Status::Different) => format!(
                "{} in this {}.",
                theta_core::text::plural(o.changes.len() as u64, "difference", "differences"),
                o.kind.label()
            ),
        };
        let head = v_flex()
            .gap_1()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} {}", o.kind.label(), o.name)))
            .child(div().text_color(colour).text_size(px(12.)).child(what));
        // A table that exists on both sides is shown as its list of differences; every other object as the
        // two texts.
        let body: AnyElement = if o.kind == ObjectKind::Table && o.status == Status::Different {
            v_flex()
                .gap_3()
                .children(o.changes.iter().map(|c| {
                    let attrs = if c.attributes.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", c.attributes.join(", "))
                    };
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(match c.status {
                                    Status::OnlyInSource => theme.green,
                                    Status::OnlyInTarget => theme.red,
                                    Status::Different => theme.yellow,
                                })
                                .child(format!("{} {} {}{attrs}", mark(c.status), c.element.label(), c.name)),
                        )
                        .child(self.side_by_side(c.source.clone(), c.target.clone(), mono, theme))
                }))
                .into_any_element()
        } else {
            self.side_by_side(o.source_text.clone(), o.target_text.clone(), mono, theme)
        };
        v_flex()
            .id("compare-detail")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(head)
            .child(body)
            .into_any_element()
    }
}
