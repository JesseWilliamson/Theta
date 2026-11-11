//! The aggregates popover: statistics of the selection, computed off the UI thread when it is large.

use crate::aggregate::{Stats, compute};
use crate::format::format_count;
use crate::grid::ResultGrid;
use crate::render::Palette;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, deferred, div, px,
};
use std::sync::Arc;
use theta_ui::{CHROME_TEXT_SIZE, tool_button};

/// Selections up to this many cells are summarised on the UI thread: faster than a round trip.
const INLINE_CELLS: usize = 20_000;

#[derive(Clone, Default)]
pub(crate) enum AggState {
    #[default]
    Closed,
    Computing,
    Ready(Stats),
}

impl ResultGrid {
    pub fn toggle_aggregates(&mut self, cx: &mut Context<Self>) {
        if matches!(self.agg, AggState::Closed) {
            self.start_aggregates(cx);
        } else {
            self.close_aggregates(cx);
        }
    }

    pub(crate) fn close_aggregates(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.agg, AggState::Closed) {
            self.agg = AggState::Closed;
            self.agg_gen += 1;
            cx.notify();
        }
    }

    pub(crate) fn aggregates_open(&self) -> bool {
        !matches!(self.agg, AggState::Closed)
    }

    /// Recompute when the popover is open and the selection (or the data) changed.
    pub(crate) fn refresh_aggregates(&mut self, cx: &mut Context<Self>) {
        if self.aggregates_open() {
            self.start_aggregates(cx);
        }
    }

    fn start_aggregates(&mut self, cx: &mut Context<Self>) {
        self.agg_gen += 1;
        let generation = self.agg_gen;
        let Some(range) = self.current_range() else {
            self.agg = AggState::Ready(Stats::default());
            cx.notify();
            return;
        };
        let staged = self.pending.as_ref().is_some_and(|p| !p.is_empty());
        if range.cell_count() <= INLINE_CELLS || staged {
            let stats = compute(
                (range.r0..=range.r1)
                    .flat_map(|r| (range.c0..=range.c1).map(move |c| (r, c)))
                    .map(|(r, c)| self.value_at(r, c)),
            );
            self.agg = AggState::Ready(stats);
            cx.notify();
            return;
        }
        // Large selection: no staged edits, so the original rows are the displayed values. Share the
        // result and the row order with a background task rather than copying cells.
        let Some(result) = self.result.clone() else { return };
        let view: Option<Arc<Vec<u32>>> = self.view.as_ref().map(|v| Arc::new(v.clone()));
        self.agg = AggState::Computing;
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_spawn(async move {
                    compute((range.r0..=range.r1).flat_map(|r| {
                        let row = view.as_ref().map_or(r, |v| v[r] as usize);
                        let result = &result;
                        (range.c0..=range.c1).filter_map(move |c| result.rows.get(row).and_then(|x| x.get(c)))
                    }))
                })
                .await;
            this.update(cx, |g, cx| {
                if g.agg_gen == generation {
                    g.agg = AggState::Ready(stats);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_aggregates(&self, cx: &mut Context<Self>, p: Palette) -> Option<AnyElement> {
        let range = self.current_range();
        let title = match range {
            Some(r) => format!("{} × {}", format_count(r.rows()), format_count(r.cols())),
            None => "No selection".to_string(),
        };
        let (rows, text): (Vec<(&'static str, String)>, Option<String>) = match &self.agg {
            AggState::Closed => return None,
            AggState::Computing => (Vec::new(), None),
            AggState::Ready(s) => (s.lines(), Some(s.to_text())),
        };
        let computing = matches!(self.agg, AggState::Computing);
        let panel = v_flex()
            .id("grid-aggregates")
            .occlude()
            .w(px(260.))
            .gap_1()
            .p_2()
            .rounded(px(6.))
            .bg(cx.theme().popover)
            .border_1()
            .border_color(p.border)
            .shadow_md()
            .text_size(CHROME_TEXT_SIZE)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(SharedString::from(format!("Selection · {title}"))),
                    )
                    .child(
                        tool_button("agg-close", IconName::X, "Close (Esc)")
                            .on_click(cx.listener(|this, _, _, cx| this.close_aggregates(cx))),
                    ),
            )
            .when(computing, |d| d.child(div().text_color(p.muted).child("Computing…")))
            .children(rows.into_iter().map(|(k, v)| {
                h_flex()
                    .justify_between()
                    .gap_3()
                    .child(div().text_color(p.muted).flex_shrink_0().child(k))
                    .child(div().truncate().child(SharedString::from(v)))
            }))
            .child(
                h_flex().justify_end().pt_1().child(
                    Button::new("agg-copy")
                        .xsmall()
                        .ghost()
                        .icon(Icon::new(IconName::Copy))
                        .label("Copy")
                        .disabled(text.is_none())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(t) = &text {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(t.clone()));
                            }
                        })),
                ),
            );
        Some(
            deferred(div().absolute().right(px(8.)).bottom(px(28.)).child(panel))
                .with_priority(10)
                .into_any_element(),
        )
    }
}
