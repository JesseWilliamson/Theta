//! A one-field dialog that asks for a name (new folder, rename). The caller supplies the validation so the
//! dialog can show the reason inline and stay open instead of closing and failing.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, px,
};
use std::rc::Rc;

/// `Ok(())` accepts the name; `Err(reason)` keeps the dialog open and shows the reason.
pub type Validate = Rc<dyn Fn(&str, &App) -> Result<(), String>>;
pub type Submit = Rc<dyn Fn(String, &mut Window, &mut App)>;

struct NameForm {
    input: Entity<InputState>,
    label: SharedString,
    submit_label: SharedString,
    validate: Validate,
    submit: Submit,
    error: Option<String>,
    _sub: Subscription,
}

pub fn open(
    title: impl Into<SharedString>,
    label: impl Into<SharedString>,
    initial: &str,
    submit_label: impl Into<SharedString>,
    validate: Validate,
    submit: Submit,
    window: &mut Window,
    cx: &mut App,
) {
    let initial = initial.to_string();
    let (label, submit_label) = (label.into(), submit_label.into());
    let form = cx.new(|cx| {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let sub =
            cx.subscribe_in(&input, window, |this: &mut NameForm, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => this.confirm(window, cx),
                InputEvent::Change if this.error.take().is_some() => cx.notify(),
                _ => {}
            });
        NameForm { input, label, submit_label, validate, submit, error: None, _sub: sub }
    });
    let title: SharedString = title.into();
    let shown = form.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog.bg(cx.theme().popover).title(title.clone()).w(px(420.)).child(shown.clone())
    });
    // The dialog takes focus while it opens; the name field should have it once it is open.
    window.defer(cx, move |window, cx| {
        let input = form.read(cx).input.clone();
        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        input.update(cx, |s, cx| s.select_all(window, cx));
    });
}

impl NameForm {
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        match (self.validate)(&text, cx) {
            Ok(()) => {
                window.close_dialog(cx);
                (self.submit)(text.trim().to_string(), window, cx);
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

impl Render for NameForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div().text_size(px(12.)).text_color(theme.muted_foreground).child(self.label.clone()),
                    )
                    .child(Input::new(&self.input))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().text_size(px(12.)).text_color(theme.danger).child(e))
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("name-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("name-ok")
                            .primary()
                            .label(self.submit_label.clone())
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            )
    }
}
