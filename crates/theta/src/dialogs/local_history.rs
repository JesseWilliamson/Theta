//! Local History: the snapshots of a console, a diff of the chosen one against the text in the editor, and
//! Restore.
//!
//! The store and its policy are `exec::local_history`, the diff is `theta_editor::diff::line_diff`; this view
//! only lists and paints. The diff goes from *your text now* to *the snapshot*: a red line is in the editor and
//! disappears if you restore, a green line comes back from the snapshot. Restoring is one undoable edit in the
//! editor, and the text it replaces is itself snapshotted first.

use crate::exec::local_history::{LocalHistory, Meta, summary};
use crate::workspace::Workspace;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, ScrollStrategy, StatefulInteractiveElement as _, Styled as _,
    UniformListScrollHandle, WeakEntity, Window, div, px, uniform_list,
};
use std::ops::Range;
use theta_editor::diff::{Diff, Op, line_diff};

/// Snapshots over this size get a size instead of a line count in the list (counting means diffing).
const COUNT_LIMIT_BYTES: u64 = 64 * 1024;

struct Row {
    meta: Meta,
    summary: String,
    /// `(added, removed)` going from the editor text to this snapshot, when cheap enough to know.
    delta: Option<(usize, usize)>,
}

pub struct LocalHistoryView {
    ws: WeakEntity<Workspace>,
    console: u64,
    store: LocalHistory,
    key: String,
    current: String,
    rows: Vec<Row>,
    sel: Option<usize>,
    text: Option<String>,
    diff: Diff,
    list_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

pub fn open(
    ws: WeakEntity<Workspace>,
    console: u64,
    key: String,
    current: String,
    store: LocalHistory,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| LocalHistoryView::new(ws, console, key, current, store, cx));
    let focus = view.clone();
    window.open_dialog(cx, {
        let view = view.clone();
        move |dialog, _, cx| {
            dialog
                .bg(cx.theme().popover)
                .title("Local History")
                .w(px(900.))
                .overlay_closable(true)
                // Enter is the dialog's OK: here it means Restore (which closes the dialog itself).
                .on_ok({
                    let view = view.clone();
                    move |_, window, cx| {
                        view.update(cx, |v, cx| v.restore(window, cx));
                        false
                    }
                })
                .child(view.clone())
        }
    });
    // The keys (up, down, enter) go to the focused view; the editor behind the dialog has it until now.
    window.defer(cx, move |window, cx| {
        let handle = focus.read(cx).focus.clone();
        window.focus(&handle, cx);
    });
}

/// `HH:MM:SS` today, `Mon D HH:MM` this year, else the full date.
fn clock(ms: u64) -> String {
    use chrono::{Datelike as _, Local, TimeZone as _};
    let Some(at) = Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    let now = Local::now();
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M:%S").to_string()
    } else if at.year() == now.year() {
        at.format("%b %-d %H:%M").to_string()
    } else {
        at.format("%Y-%m-%d %H:%M").to_string()
    }
}

impl LocalHistoryView {
    fn new(
        ws: WeakEntity<Workspace>,
        console: u64,
        key: String,
        current: String,
        store: LocalHistory,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows: Vec<Row> = store
            .list(&key)
            .into_iter()
            .map(|meta| {
                let text = store.read(&key, &meta.id);
                let delta = text
                    .as_deref()
                    .filter(|_| meta.bytes <= COUNT_LIMIT_BYTES)
                    .map(|t| line_diff(&current, t))
                    .map(|d| (d.added(), d.removed()));
                Row { summary: text.as_deref().map(summary).unwrap_or_default(), delta, meta }
            })
            .collect();
        let mut v = Self {
            ws,
            console,
            store,
            key,
            current,
            sel: None,
            text: None,
            diff: Diff::default(),
            list_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            rows,
        };
        // Start on the newest snapshot that differs from the editor: the one most likely wanted.
        let first = v.rows.iter().position(|r| r.delta != Some((0, 0))).or((!v.rows.is_empty()).then_some(0));
        v.select(first);
        v
    }

    fn select(&mut self, i: Option<usize>) {
        self.sel = i;
        self.text = i.and_then(|i| self.store.read(&self.key, &self.rows[i].meta.id));
        self.diff = self.text.as_deref().map(|t| line_diff(&self.current, t)).unwrap_or_default();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let at = self.sel.unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        self.select(Some(next));
        self.list_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.text.clone() else { return };
        if let Some(ws) = self.ws.upgrade() {
            let console = self.console;
            ws.update(cx, |w, cx| w.restore_snapshot(console, text, window, cx));
        }
        window.close_dialog(cx);
    }
}

impl Render for LocalHistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let empty = self.rows.is_empty();
        let can_restore = self.text.is_some();
        let identical = self.text.is_some() && self.diff.is_identical();
        v_flex()
            .gap_2()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                match ev.keystroke.key.as_str() {
                    "down" | "j" => this.step(1, cx),
                    "up" | "k" => this.step(-1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(div().text_size(px(11.)).text_color(theme.muted_foreground).child(
                "Red lines are in the editor now and go away on Restore; green lines come back from the snapshot. Restore is one undo step.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .h(px(380.))
                    .child(
                        div().w(px(330.)).flex_shrink_0().h_full().border_1().border_color(theme.border).rounded_md().child(
                            if empty {
                                div()
                                    .p_3()
                                    .text_size(px(12.))
                                    .text_color(theme.muted_foreground)
                                    .child("No snapshots yet. One is taken whenever a statement is run and after a pause in typing.")
                                    .into_any_element()
                            } else {
                                uniform_list(
                                    "lh-rows",
                                    self.rows.len(),
                                    cx.processor(|this, range: Range<usize>, _w, cx| {
                                        let theme = cx.theme().clone();
                                        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
                                        range
                                            .map(|i| {
                                                let r = &this.rows[i];
                                                let selected = this.sel == Some(i);
                                                v_flex()
                                                    .id(("lh-row", i))
                                                    .w_full()
                                                    .h(px(44.))
                                                    .px_2()
                                                    .py_1()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .when(selected, |d| d.bg(theme.list_active))
                                                    .when(!selected, |d| d.hover(|s| s.bg(theme.list_hover)))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.select(Some(i));
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .text_size(px(11.))
                                                            .child(div().font_weight(FontWeight::MEDIUM).child(clock(r.meta.at_ms)))
                                                            .child(div().text_color(theme.muted_foreground).child(r.meta.kind.label()))
                                                            .child(div().flex_1())
                                                            .child(match r.delta {
                                                                Some((0, 0)) => div().text_color(theme.muted_foreground).child("same as now"),
                                                                Some((a, d)) => div()
                                                                    .child(div().text_color(theme.success).child(format!("+{a}")))
                                                                    .child(div().text_color(theme.danger).ml_1().child(format!("−{d}")))
                                                                    .flex()
                                                                    .flex_row(),
                                                                None => div().text_color(theme.muted_foreground).child(format!("{} KB", r.meta.bytes / 1024 + 1)),
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .font_family(mono.clone())
                                                            .truncate()
                                                            .child(if r.summary.is_empty() { "(blank)".to_string() } else { r.summary.clone() }),
                                                    )
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .track_scroll(&self.list_scroll)
                                .size_full()
                                .into_any_element()
                            },
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().h_full().border_1().border_color(theme.border).rounded_md().child(
                            if self.text.is_some() {
                                if identical {
                                    div()
                                        .p_3()
                                        .text_size(px(12.))
                                        .text_color(theme.muted_foreground)
                                        .child("This snapshot is the same as the text in the editor.")
                                        .into_any_element()
                                } else {
                                    let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
                                    v_flex()
                                        .size_full()
                                        .when(self.diff.coarse, |d| {
                                            d.child(div().px_2().py_1().text_size(px(11.)).text_color(theme.warning).child(
                                                "The change is too large to compare line by line; the whole changed section is shown as removed and added.",
                                            ))
                                        })
                                        .child(
                                            uniform_list(
                                                "lh-diff",
                                                self.diff.lines.len(),
                                                cx.processor(move |this, range: Range<usize>, _w, cx| {
                                                    let theme = cx.theme().clone();
                                                    range
                                                        .map(|i| {
                                                            let l = &this.diff.lines[i];
                                                            let (mark, bg, fg) = match l.op {
                                                                Op::Same => (" ", None, theme.muted_foreground),
                                                                Op::Removed => ("−", Some(theme.danger.opacity(0.16)), theme.foreground),
                                                                Op::Added => ("+", Some(theme.success.opacity(0.16)), theme.foreground),
                                                            };
                                                            h_flex()
                                                                .h(px(18.))
                                                                .px_2()
                                                                .gap_2()
                                                                .font_family(mono.clone())
                                                                .text_size(px(12.))
                                                                .text_color(fg)
                                                                .when_some(bg, |d, c| d.bg(c))
                                                                .whitespace_nowrap()
                                                                .child(div().w(px(10.)).flex_shrink_0().text_color(theme.muted_foreground).child(mark))
                                                                .child(l.text.clone())
                                                        })
                                                        .collect::<Vec<_>>()
                                                }),
                                            )
                                            .track_scroll(&self.diff_scroll)
                                            .flex_1(),
                                        )
                                        .into_any_element()
                                }
                            } else {
                                div().into_any_element()
                            },
                        ),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("lh-restore")
                            .primary()
                            .label("Restore")
                            .disabled(!can_restore || identical)
                            .on_click(cx.listener(|this, _, w, cx| this.restore(w, cx))),
                    )
                    .child(Button::new("lh-close").label("Close").on_click(|_, w, cx| w.close_dialog(cx))),
            )
    }
}
