//! History tab: every executed statement, searchable.

use crate::exec::history::{HistoryEntry, HistoryStore};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    FocusHandle, KeyDownEvent, ScrollStrategy, UniformListScrollHandle, Window, div, px, uniform_list,
};
use std::ops::Range;
use theta_ui::{CHROME_TEXT_SIZE, EmptyState};

#[derive(Debug, Clone, PartialEq)]
pub enum HistoryEvent {
    /// Insert into the active console.
    Insert(String),
    OpenNew {
        connection: String,
        sql: String,
    },
    /// Escape in the list: back to the editor.
    Leave,
}

pub struct HistoryPanel {
    store: HistoryStore,
    filter: Entity<InputState>,
    query: String,
    shown: Vec<HistoryEntry>,
    scroll: UniformListScrollHandle,
    selected: Option<usize>,
    /// The list's own focus, so the keys below act only when the list (not the search box) has it.
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<HistoryEvent> for HistoryPanel {}

/// Local time of a history entry: just the clock for today, the date too for older ones.
fn clock(ms: u64) -> String {
    use chrono::{Local, TimeZone as _};
    let Some(at) = Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    format_local(at, Local::now())
}

fn format_local<Tz: chrono::TimeZone>(at: chrono::DateTime<Tz>, now: chrono::DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    use chrono::Datelike as _;
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M:%S").to_string()
    } else if at.year() == now.year() {
        at.format("%b %-d %H:%M").to_string()
    } else {
        at.format("%Y-%m-%d %H:%M").to_string()
    }
}

impl HistoryPanel {
    pub fn new(store: HistoryStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search history"));
        let sub = cx.subscribe_in(&filter, window, |this, input, e: &InputEvent, window, cx| match e {
            InputEvent::Change => {
                this.query = input.read(cx).value().to_string();
                this.refresh(cx);
            }
            // Enter in the search box moves on to the matches.
            InputEvent::PressEnter { .. } => {
                this.select(0, cx);
                window.focus(&this.focus, cx);
            }
            _ => {}
        });
        let mut me = Self {
            store,
            filter,
            query: String::new(),
            shown: vec![],
            scroll: UniformListScrollHandle::new(),
            selected: None,
            focus: cx.focus_handle(),
            _subs: vec![sub],
        };
        me.shown = me.store.search("").into_iter().cloned().collect();
        me
    }

    pub fn push(&mut self, e: HistoryEntry, cx: &mut Context<Self>) {
        self.store.push(e);
        self.refresh(cx);
    }

    pub fn len(&self) -> usize {
        self.store.entries().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub fn last_sql(&self) -> Option<&str> {
        self.store.entries().last().map(|e| e.sql.as_str())
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Take the keyboard; the newest statement (the first row) is current unless one already is.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none() && !self.shown.is_empty() {
            self.select(0, cx);
        }
        window.focus(&self.focus, cx);
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let ix = ix.min(self.shown.len() - 1);
        self.selected = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let at = self.selected.map_or(if delta > 0 { -1 } else { 0 }, |i| i as isize);
        self.select((at + delta).clamp(0, self.shown.len() as isize - 1) as usize, cx);
    }

    /// Up, down, home, end (and `j` / `k` in the modal keymaps) walk the statements, `enter` loads the
    /// current one into the editor (run it with the usual key from there), `/` goes to the search box and
    /// `escape` returns to the editor.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The search box is a child whose keystrokes bubble up here.
        if !self.focus.is_focused(window) {
            return;
        }
        let m = ev.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        let modal = theta_ui::Settings::get(cx).keymap.is_modal();
        match ev.keystroke.key.as_str() {
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "j" if modal => self.step(1, cx),
            "k" if modal => self.step(-1, cx),
            "home" => self.select(0, cx),
            "end" => self.select(usize::MAX, cx),
            "pageup" => self.step(-10, cx),
            "pagedown" => self.step(10, cx),
            "enter" => match self.selected.and_then(|i| self.shown.get(i)) {
                Some(e) => cx.emit(HistoryEvent::Insert(e.sql.clone())),
                None => return,
            },
            "/" => self.filter.update(cx, |s, cx| s.focus(window, cx)),
            "escape" => cx.emit(HistoryEvent::Leave),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.shown = self.store.search(&self.query).into_iter().cloned().collect();
        self.selected = None;
        cx.notify();
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();
        let Some(e) = self.shown.get(ix).cloned() else { return div().into_any_element() };
        let selected = self.selected == Some(ix);
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        let first = crate::exec::pick::first_line(&e.sql);
        let sql = e.sql.clone();
        let sql2 = e.sql.clone();
        let conn = e.connection.clone();
        let panel = cx.entity().downgrade();
        h_flex()
            .id(("history", ix))
            // The whole row, not the width of its text, carries the selection.
            .w_full()
            .h(px(24.))
            .px_2()
            .gap_2()
            .items_center()
            .text_size(CHROME_TEXT_SIZE)
            .when(selected, |d| d.bg(theme.list_active))
            .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
            .child(
                Icon::new(if e.ok { IconName::Check } else { IconName::CircleAlert })
                    .size(px(12.))
                    .text_color(if e.ok { theme.success } else { theme.danger }),
            )
            .child(
                div()
                    .w(px(96.))
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(theme.muted_foreground)
                    .child(clock(e.at)),
            )
            .child(
                div()
                    .w(px(170.))
                    .flex_shrink_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(e.connection.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .font_family(mono)
                    .text_size(px(12.))
                    .child(SharedString::from(first)),
            )
            .child(
                div()
                    .w(px(130.))
                    .flex_shrink_0()
                    .text_color(theme.muted_foreground)
                    .whitespace_nowrap()
                    .child(match e.rows {
                        Some(r) => format!(
                            "{} · {}",
                            crate::workspace::console::plural(r, "row", "rows"),
                            theta_core::text::duration(std::time::Duration::from_millis(e.duration_ms))
                        ),
                        None => theta_core::text::duration(std::time::Duration::from_millis(e.duration_ms)),
                    }),
            )
            .on_click(cx.listener(move |this, ev: &gpui_kit::ClickEvent, window, cx| {
                this.selected = Some(ix);
                window.focus(&this.focus, cx);
                if ev.click_count() >= 2 {
                    cx.emit(HistoryEvent::Insert(sql.clone()));
                }
                cx.notify();
            }))
            .context_menu(move |menu, _, _| {
                let (p1, p2) = (panel.clone(), panel.clone());
                let (s1, s2, s3) = (sql2.clone(), sql2.clone(), sql2.clone());
                let c = conn.clone();
                menu.item(
                    PopupMenuItem::new("Insert into Console")
                        .icon(Icon::new(IconName::CornerDownLeft))
                        .on_click(move |_, _, cx| {
                            p1.update(cx, |_, cx| cx.emit(HistoryEvent::Insert(s1.clone()))).ok();
                        }),
                )
                .item(
                    PopupMenuItem::new("Open in New Console")
                        .icon(Icon::new(IconName::SquareTerminal))
                        .on_click(move |_, _, cx| {
                            p2.update(cx, |_, cx| {
                                cx.emit(HistoryEvent::OpenNew { connection: c.clone(), sql: s2.clone() })
                            })
                            .ok();
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy").icon(Icon::new(IconName::Copy)).on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(s3.clone()))
                    }),
                )
            })
            .into_any_element()
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.shown.len();
        let body = if self.store.entries().is_empty() {
            EmptyState::new(IconName::Clock, "No history yet")
                .description("Statements you run are kept here so you can run them again.")
                .into_any_element()
        } else if count == 0 {
            EmptyState::new(IconName::Search, "No matches")
                .description("Nothing in the history matches.")
                .into_any_element()
        } else {
            uniform_list(
                "history-rows",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| {
                    range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        v_flex()
            .id("history")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex_shrink_0()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Input::new(&self.filter)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size(px(13.))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}

#[cfg(test)]
mod clock_tests {
    use super::format_local;
    use chrono::{FixedOffset, TimeZone as _};

    #[test]
    fn today_shows_the_clock_older_entries_the_date() {
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = tz.with_ymd_and_hms(2026, 11, 10, 15, 0, 0).unwrap();
        let today = tz.with_ymd_and_hms(2026, 11, 10, 14, 32, 7).unwrap();
        assert_eq!(format_local(today, now), "14:32:07");
        let earlier = tz.with_ymd_and_hms(2026, 3, 4, 9, 5, 0).unwrap();
        assert_eq!(format_local(earlier, now), "Mar 4 09:05");
        let last_year = tz.with_ymd_and_hms(2025, 12, 31, 23, 59, 0).unwrap();
        assert_eq!(format_local(last_year, now), "2025-12-31 23:59");
    }
}
