//! The Zed-style modal fuzzy finder.
//!
//! Every "type to filter, pick one" surface in the app (command palette, theme
//! picker, connection picker, table picker) is a [`PickerDelegate`] behind this
//! one view, so they all look and behave identically: same width and position,
//! same keys, same highlighting, same empty state.
//!
//! The picker renders its own backdrop and panel (no gpui-kit `Dialog`, whose
//! title bar and padding are wrong for a finder). The host window mounts it as
//! the last child of its root element and removes it on [`PickerEvent::Dismiss`].

use crate::fuzzy::{fuzzy_match, highlight_ranges};
use crate::keyhint::KeyHint;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, HighlightStyle,
    InteractiveElement as _, IntoElement, KeyBinding, MouseButton, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Subscription,
    UniformListScrollHandle, Window, actions, div, px, uniform_list,
};
use std::ops::Range;

actions!(quarry_picker, [PickerNext, PickerPrev, PickerFirst, PickerLast]);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", PickerNext, Some("Picker")),
        KeyBinding::new("ctrl-p", PickerPrev, Some("Picker")),
        KeyBinding::new("pagedown", PickerNext, Some("Picker")),
        KeyBinding::new("pageup", PickerPrev, Some("Picker")),
        KeyBinding::new("ctrl-home", PickerFirst, Some("Picker")),
        KeyBinding::new("ctrl-end", PickerLast, Some("Picker")),
    ]);
}

pub const PICKER_WIDTH: f32 = 560.0;
const ROW_H: f32 = 28.0;
const MAX_VISIBLE_ROWS: usize = 10;

#[derive(Clone)]
pub struct PickerItem {
    /// Stable identity handed back to the delegate.
    pub id: SharedString,
    /// Matched against the query and shown (with matched characters highlighted).
    pub label: SharedString,
    /// Muted right-aligned text (e.g. `public`, a connection kind).
    pub detail: Option<SharedString>,
    /// Pre-formatted key binding shown as a chip (see `format_keystrokes`).
    pub key_hint: Option<SharedString>,
    pub icon: Option<IconName>,
    /// Section header shown while the query is empty.
    pub section: Option<SharedString>,
    /// Draws a check mark (current theme, active connection).
    pub checked: bool,
}

impl PickerItem {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            key_hint: None,
            icon: None,
            section: None,
            checked: false,
        }
    }
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn key_hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.key_hint = Some(hint.into());
        self
    }
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn section(mut self, section: impl Into<SharedString>) -> Self {
        self.section = Some(section.into());
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// What a concrete picker supplies. The picker owns query, filtering, keys and
/// painting; the delegate owns the data and what happens next.
pub trait PickerDelegate: 'static {
    fn placeholder(&self) -> SharedString {
        "Search…".into()
    }
    fn empty_message(&self) -> SharedString {
        "No matches".into()
    }
    /// All candidates, in display order. Called once when the picker opens.
    fn items(&mut self, cx: &mut App) -> Vec<PickerItem>;
    /// Index into `items` to highlight first (e.g. the current theme).
    fn initial_selection(&self, _items: &[PickerItem]) -> Option<usize> {
        None
    }
    /// The highlighted item changed through keyboard or pointer: live preview hook.
    fn selection_changed(&mut self, _item: &PickerItem, _cx: &mut App) {}
    /// Enter or click on `item`.
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App);
    /// The picker is closing; `confirmed` is false for Escape/click-away, so
    /// previewing delegates can revert here.
    fn dismissed(&mut self, _confirmed: bool, _window: &mut Window, _cx: &mut App) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerEvent {
    Dismiss,
}

enum Row {
    Header(SharedString),
    Item { item: usize, highlights: Vec<Range<usize>> },
}

pub struct Picker<D: PickerDelegate> {
    delegate: D,
    input: Entity<InputState>,
    items: Vec<PickerItem>,
    rows: Vec<Row>,
    /// Row index of every selectable (non-header) row.
    selectable: Vec<usize>,
    /// Position within `selectable`.
    selected: usize,
    scroll: UniformListScrollHandle,
    query: String,
    finished: bool,
    _subscriptions: Vec<Subscription>,
}

impl<D: PickerDelegate> EventEmitter<PickerEvent> for Picker<D> {}

impl<D: PickerDelegate> Picker<D> {
    pub fn new(mut delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let items = delegate.items(cx);
        let placeholder = delegate.placeholder();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.query = input.read(cx).value().to_string();
                    this.refilter(cx);
                    this.preview_selection(cx);
                }
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                _ => {}
            });
        let initial = delegate.initial_selection(&items);
        let mut picker = Self {
            delegate,
            input: input.clone(),
            items,
            rows: Vec::new(),
            selectable: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            query: String::new(),
            finished: false,
            _subscriptions: vec![subscription],
        };
        picker.refilter(cx);
        if let Some(i) = initial
            && let Some(pos) = picker
                .selectable
                .iter()
                .position(|&r| matches!(picker.rows[r], Row::Item { item, .. } if item == i))
        {
            picker.selected = pos;
            picker.scroll_to_selected();
        }
        input.update(cx, |state, cx| state.focus(window, cx));
        picker
    }

    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    /// Recompute rows for the current query. With an empty query the delegate's
    /// order and section headers are kept; otherwise results are ranked by score.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        self.selectable.clear();
        if self.query.trim().is_empty() {
            let mut section: Option<&SharedString> = None;
            for (i, item) in self.items.iter().enumerate() {
                if item.section.as_ref() != section {
                    section = item.section.as_ref();
                    if let Some(s) = section {
                        self.rows.push(Row::Header(s.clone()));
                    }
                }
                self.selectable.push(self.rows.len());
                self.rows.push(Row::Item { item: i, highlights: Vec::new() });
            }
        } else {
            let mut matches: Vec<(usize, i64, Vec<usize>)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    fuzzy_match(&self.query, &item.label).map(|m| (i, m.score, m.positions))
                })
                .collect();
            matches.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (i, _, positions) in matches {
                let highlights = highlight_ranges(&self.items[i].label, &positions);
                self.selectable.push(self.rows.len());
                self.rows.push(Row::Item { item: i, highlights });
            }
        }
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn selected_item(&self) -> Option<PickerItem> {
        let row = *self.selectable.get(self.selected)?;
        match &self.rows[row] {
            Row::Item { item, .. } => self.items.get(*item).cloned(),
            Row::Header(_) => None,
        }
    }

    fn scroll_to_selected(&self) {
        if let Some(&row) = self.selectable.get(self.selected) {
            // When the first item sits under a header, show the header too.
            let target = if self.selected == 0 { 0 } else { row };
            self.scroll.scroll_to_item(target, ScrollStrategy::Nearest);
        }
    }

    fn preview_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_item() {
            self.delegate.selection_changed(&item, cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.selectable.len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
        self.scroll_to_selected();
        self.preview_selection(cx);
        cx.notify();
    }

    fn select_index(&mut self, position: usize, cx: &mut Context<Self>) {
        if position != self.selected && position < self.selectable.len() {
            self.selected = position;
            self.preview_selection(cx);
            cx.notify();
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        let Some(item) = self.selected_item() else { return };
        self.finished = true;
        self.delegate.confirm(&item, window, cx);
        self.delegate.dismissed(true, window, cx);
        cx.emit(PickerEvent::Dismiss);
    }

    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.delegate.dismissed(false, window, cx);
        cx.emit(PickerEvent::Dismiss);
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        match &self.rows[ix] {
            Row::Header(text) => h_flex()
                .h(px(ROW_H))
                .px_3()
                .items_end()
                .pb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(text.clone())
                .into_any_element(),
            Row::Item { item, highlights } => {
                let item = &self.items[*item];
                let position = self.selectable.iter().position(|&r| r == ix).unwrap_or(0);
                let selected = position == self.selected;
                let styled =
                    StyledText::new(item.label.clone()).with_highlights(highlights.iter().map(|r| {
                        (
                            r.clone(),
                            HighlightStyle {
                                color: Some(theme.ring),
                                font_weight: Some(FontWeight::SEMIBOLD),
                                ..Default::default()
                            },
                        )
                    }));
                h_flex()
                    .id(SharedString::from(format!("picker-row-{}", item.id)))
                    .h(px(ROW_H))
                    .mx_1()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(px(4.))
                    .cursor_default()
                    .when(selected, |this| this.bg(theme.list_active))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| this.select_index(position, cx)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_index(position, cx);
                        this.confirm(window, cx);
                    }))
                    .when_some(item.icon, |this, icon| {
                        this.child(Icon::new(icon).size(px(14.)).text_color(theme.muted_foreground))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .text_color(theme.foreground)
                            .child(styled),
                    )
                    .when_some(item.detail.clone(), |this, d| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(d),
                        )
                    })
                    .when_some(item.key_hint.clone(), |this, k| this.child(KeyHint::from_label(k)))
                    .when(item.checked, |this| {
                        this.child(Icon::new(IconName::Check).size(px(14.)).text_color(theme.ring))
                    })
                    .into_any_element()
            }
        }
    }
}

impl<D: PickerDelegate> Render for Picker<D> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows = self.rows.len();
        let list_height = px(ROW_H * rows.min(MAX_VISIBLE_ROWS) as f32 + 8.);
        let empty = self.selectable.is_empty();
        let message = self.delegate.empty_message();

        div()
            .id("picker-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme.overlay)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.cancel(window, cx)))
            .child(
                v_flex().key_context("Picker").w_full().items_center().pt(px(72.)).px_4().child(
                    v_flex()
                        .id("picker-panel")
                        .w(px(PICKER_WIDTH))
                        .max_w_full()
                        .overflow_hidden()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.popover)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                            this.move_selection(-1, cx);
                            cx.stop_propagation();
                        }))
                        .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                            this.move_selection(1, cx);
                            cx.stop_propagation();
                        }))
                        .on_action(cx.listener(|this, _: &PickerNext, _, cx| this.move_selection(1, cx)))
                        .on_action(cx.listener(|this, _: &PickerPrev, _, cx| this.move_selection(-1, cx)))
                        .on_action(cx.listener(|this, _: &PickerFirst, _, cx| {
                            this.selected = 0;
                            this.scroll_to_selected();
                            this.preview_selection(cx);
                            cx.notify();
                        }))
                        .on_action(cx.listener(|this, _: &PickerLast, _, cx| {
                            this.selected = this.selectable.len().saturating_sub(1);
                            this.scroll_to_selected();
                            this.preview_selection(cx);
                            cx.notify();
                        }))
                        .on_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                            this.cancel(window, cx)
                        }))
                        .child(
                            h_flex()
                                .h(px(40.))
                                .px_3()
                                .items_center()
                                .border_b_1()
                                .border_color(theme.border)
                                .child(
                                    Input::new(&self.input)
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false),
                                ),
                        )
                        .when(empty, |this| {
                            this.child(
                                div()
                                    .h(px(64.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(13.))
                                    .text_color(theme.muted_foreground)
                                    .child(message),
                            )
                        })
                        .when(!empty, |this| {
                            this.child(
                                div().h(list_height).py_1().child(
                                    uniform_list(
                                        "picker-items",
                                        rows,
                                        cx.processor(|this, range: Range<usize>, _window, cx| {
                                            range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                                        }),
                                    )
                                    .track_scroll(&self.scroll)
                                    .size_full(),
                                ),
                            )
                        }),
                ),
            )
    }
}
