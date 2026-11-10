//! A small confirmation dialog that can show SQL (monospace, scrollable) and
//! offers one or more action buttons next to Cancel.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Primary,
    Danger,
    Plain,
}

pub type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub tone: Tone,
    pub run: Callback,
}

impl Choice {
    pub fn new(label: impl Into<String>, tone: Tone, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { label: label.into(), tone, run: Rc::new(run) }
    }
}

struct ConfirmView {
    intro: SharedString,
    statements: Vec<String>,
    choices: Vec<Choice>,
}

impl Render for ConfirmView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = quarry_ui::Settings::get(cx).buffer_font_family.clone();
        v_flex()
            .gap_3()
            .child(div().text_size(px(13.)).text_color(theme.foreground).child(self.intro.clone()))
            .when(!self.statements.is_empty(), |d| {
                d.child(
                    div()
                        .id("confirm-sql")
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .border_1()
                        .border_color(theme.border)
                        .font_family(mono)
                        .text_size(px(12.))
                        .child(v_flex().gap_1().children(
                            self.statements.iter().map(|s| div().child(SharedString::from(format!("{s};")))),
                        )),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("confirm-cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .children(self.choices.iter().enumerate().map(|(i, c)| {
                        let run = c.run.clone();
                        let b = Button::new(("confirm-choice", i)).label(SharedString::from(c.label.clone()));
                        let b = match c.tone {
                            Tone::Primary => b.primary(),
                            Tone::Danger => b.danger(),
                            Tone::Plain => b,
                        };
                        b.on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            run(window, cx);
                        })
                    })),
            )
    }
}

pub fn open(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    intro: impl Into<SharedString>,
    statements: Vec<String>,
    choices: Vec<Choice>,
) {
    let view = cx.new(|_| ConfirmView { intro: intro.into(), statements, choices });
    let title: SharedString = title.into();
    window.open_dialog(cx, move |dialog, _, _| dialog.title(title.clone()).w(px(640.)).child(view.clone()));
}
