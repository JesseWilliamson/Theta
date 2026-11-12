//! The "Parameters" dialog: one row per distinct placeholder of the statements about to run.
//!
//! Values are substituted client-side as literals (`theta_sql::params`), so each row has a type that
//! decides how the text becomes SQL. The dialog validates every row with the same function the
//! substitution uses, so a value that would fail never closes the dialog.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use std::rc::Rc;
use theta_core::DatabaseKind;
use theta_sql::params::{ParamKey, ParamType, ParamValue, literal};

pub type Submit = Rc<dyn Fn(Vec<ParamValue>, &mut Window, &mut App)>;

struct Row {
    label: SharedString,
    value: Entity<InputState>,
    ty: Entity<SelectState<Vec<&'static str>>>,
}

struct ParamsForm {
    rows: Vec<Row>,
    kind: DatabaseKind,
    error: Option<String>,
    submit: Submit,
    note: SharedString,
    _subs: Vec<Subscription>,
}

/// Open the dialog. `initial` holds the remembered value of each slot (same order as `slots`).
pub fn open(
    slots: &[ParamKey],
    initial: Vec<ParamValue>,
    kind: DatabaseKind,
    note: impl Into<SharedString>,
    submit: Submit,
    window: &mut Window,
    cx: &mut App,
) {
    let slots = slots.to_vec();
    let note = note.into();
    let form = cx.new(|cx| ParamsForm::new(&slots, initial, kind, note, submit, window, cx));
    let form_for_focus = form.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog.bg(cx.theme().popover).title("Parameters").w(px(560.)).child(form.clone())
    });
    // The dialog moves focus to itself while it opens; take it back for the first value once it has.
    window.defer(cx, move |window, cx| {
        let first = form_for_focus.read(cx).rows.first().map(|r| r.value.clone());
        if let Some(first) = first {
            let handle = first.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            first.update(cx, |s, cx| s.select_all(window, cx));
        }
    });
}

impl ParamsForm {
    fn new(
        slots: &[ParamKey],
        initial: Vec<ParamValue>,
        kind: DatabaseKind,
        note: SharedString,
        submit: Submit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let labels: Vec<&'static str> = ParamType::ALL.iter().map(|t| t.label()).collect();
        let mut subs = Vec::new();
        let rows: Vec<Row> = slots
            .iter()
            .zip(initial)
            .map(|(key, init)| {
                let text = init.text.clone();
                let value = cx.new(|cx| InputState::new(window, cx).default_value(text));
                let ix = ParamType::ALL.iter().position(|t| *t == init.ty).unwrap_or(0);
                let ty = cx.new(|cx| SelectState::new(labels.clone(), Some(IndexPath::new(ix)), window, cx));
                subs.push(cx.subscribe_in(&value, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.confirm(window, cx);
                    }
                }));
                subs.push(cx.subscribe_in(
                    &ty,
                    window,
                    |this, _, _: &SelectEvent<Vec<&'static str>>, _, cx| {
                        this.error = None;
                        cx.notify();
                    },
                ));
                Row { label: key.label().into(), value, ty }
            })
            .collect();
        Self { rows, kind, error: None, submit, note, _subs: subs }
    }

    fn values(&self, cx: &App) -> Vec<ParamValue> {
        self.rows
            .iter()
            .map(|r| {
                let ty =
                    r.ty.read(cx)
                        .selected_index(cx)
                        .and_then(|ix| ParamType::ALL.get(ix.row).copied())
                        .unwrap_or_default();
                ParamValue { ty, text: r.value.read(cx).value().to_string() }
            })
            .collect()
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        for (row, v) in self.rows.iter().zip(&values) {
            if let Err(e) = literal(v, self.kind) {
                self.error = Some(format!("{}: {e}", row.label));
                cx.notify();
                return;
            }
        }
        window.close_dialog(cx);
        (self.submit)(values, window, cx);
    }
}

impl Render for ParamsForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        v_flex()
            .gap_3()
            .child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(self.note.clone()))
            .child(div().id("params-rows").max_h(px(360.)).overflow_y_scroll().child(
                v_flex().gap_2().children(self.rows.iter().map(|r| {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .w(px(96.))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .font_family(mono.clone())
                                .text_size(px(13.))
                                .child(r.label.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(Input::new(&r.value)))
                        .child(div().w(px(112.)).flex_shrink_0().child(Select::new(&r.ty)))
                })),
            ))
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_size(px(12.)).text_color(theme.danger).child(e))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("params-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("params-run")
                            .primary()
                            .label("Run")
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            )
    }
}
