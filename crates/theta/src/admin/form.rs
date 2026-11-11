//! A small form dialog for the Roles tab: some fields, a live preview of the SQL they produce, and a button
//! that hands the result to a confirmation.
//!
//! The form knows nothing about roles. It is given fields and a `build` function from the field values to a
//! [`Change`] (or an error to show under the fields), which is `theta_sql::admin::roles` in practice. The
//! preview shows `Change::shown`, so a password is never on screen as typed; the real statement is only
//! handed to `on_run`, which executes it.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use std::collections::HashMap;
use std::rc::Rc;
use theta_sql::admin::roles::Change;
use theta_ui::Settings;

pub enum FieldKind {
    Text { initial: String, placeholder: String, masked: bool },
    Check { initial: bool },
    Choice { options: Vec<String>, initial: usize },
}

pub struct Field {
    pub id: &'static str,
    pub label: String,
    pub kind: FieldKind,
}

impl Field {
    pub fn text(id: &'static str, label: &str, initial: &str, placeholder: &str) -> Field {
        Field {
            id,
            label: label.into(),
            kind: FieldKind::Text { initial: initial.into(), placeholder: placeholder.into(), masked: false },
        }
    }

    pub fn secret(id: &'static str, label: &str) -> Field {
        Field {
            id,
            label: label.into(),
            kind: FieldKind::Text { initial: String::new(), placeholder: String::new(), masked: true },
        }
    }

    pub fn check(id: &'static str, label: &str, initial: bool) -> Field {
        Field { id, label: label.into(), kind: FieldKind::Check { initial } }
    }

    pub fn choice(id: &'static str, label: &str, options: Vec<String>, initial: usize) -> Field {
        Field { id, label: label.into(), kind: FieldKind::Choice { options, initial } }
    }
}

/// The current value of every field, by id.
#[derive(Default, Clone)]
pub struct Values {
    text: HashMap<&'static str, String>,
    flag: HashMap<&'static str, bool>,
}

impl Values {
    pub fn text(&self, id: &str) -> &str {
        self.text.get(id).map(String::as_str).unwrap_or("")
    }

    pub fn flag(&self, id: &str) -> bool {
        self.flag.get(id).copied().unwrap_or(false)
    }

    /// A choice's selected label.
    pub fn choice(&self, id: &str) -> &str {
        self.text(id)
    }
}

pub type Build = Rc<dyn Fn(&Values) -> Result<Change, String>>;
pub type Run = Rc<dyn Fn(Change, &mut Window, &mut App)>;

pub struct FormSpec {
    pub title: String,
    pub intro: Option<String>,
    pub fields: Vec<Field>,
    pub submit: String,
    pub build: Build,
    pub on_run: Run,
}

enum Control {
    Text(Entity<InputState>),
    Check(bool),
    Choice(Entity<SelectState<Vec<String>>>),
}

struct FormView {
    intro: Option<SharedString>,
    fields: Vec<(&'static str, SharedString, Control)>,
    submit: SharedString,
    build: Build,
    on_run: Run,
    _subs: Vec<Subscription>,
}

pub fn open(spec: FormSpec, window: &mut Window, cx: &mut App) {
    let title: SharedString = spec.title.clone().into();
    let view = cx.new(|cx| FormView::new(spec, window, cx));
    let first = view.clone();
    let shown = view.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        dialog
            .bg(cx.theme().popover)
            .title(title.clone())
            .w(px(560.))
            .overlay_closable(false)
            // Enter belongs to the fields; running is a button.
            .on_ok(|_, _, _| false)
            .child(shown.clone())
    });
    window.defer(cx, move |window, cx| {
        let input = first.read(cx).fields.iter().find_map(|(_, _, c)| match c {
            Control::Text(i) => Some(i.clone()),
            _ => None,
        });
        if let Some(i) = input {
            i.update(cx, |s, cx| s.focus(window, cx));
        }
    });
}

impl FormView {
    fn new(spec: FormSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subs = Vec::new();
        let mut fields = Vec::new();
        for f in spec.fields {
            let control = match f.kind {
                FieldKind::Text { initial, placeholder, masked } => {
                    let input = cx.new(|cx| {
                        let mut s =
                            InputState::new(window, cx).default_value(initial).placeholder(placeholder);
                        if masked {
                            s.set_masked(true, window, cx);
                        }
                        s
                    });
                    subs.push(cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
                        match ev {
                            InputEvent::Change => cx.notify(),
                            // Enter in a field runs the form when it is valid, so a form is fillable from the keyboard.
                            InputEvent::PressEnter { .. } => this.run(window, cx),
                            _ => {}
                        }
                    }));
                    Control::Text(input)
                }
                FieldKind::Check { initial } => Control::Check(initial),
                FieldKind::Choice { options, initial } => {
                    let sel =
                        cx.new(|cx| SelectState::new(options, Some(IndexPath::new(initial)), window, cx));
                    subs.push(
                        cx.subscribe_in(&sel, window, |_, _, _: &SelectEvent<Vec<String>>, _, cx| {
                            cx.notify()
                        }),
                    );
                    Control::Choice(sel)
                }
            };
            fields.push((f.id, SharedString::from(f.label), control));
        }
        FormView {
            intro: spec.intro.map(Into::into),
            fields,
            submit: spec.submit.into(),
            build: spec.build,
            on_run: spec.on_run,
            _subs: subs,
        }
    }

    fn values(&self, cx: &App) -> Values {
        let mut v = Values::default();
        for (id, _, c) in &self.fields {
            match c {
                Control::Text(i) => {
                    v.text.insert(id, i.read(cx).value().to_string());
                }
                Control::Check(b) => {
                    v.flag.insert(id, *b);
                }
                Control::Choice(s) => {
                    v.text.insert(id, s.read(cx).selected_value().cloned().unwrap_or_default());
                }
            }
        }
        v
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        if let Ok(change) = (self.build)(&values) {
            window.close_dialog(cx);
            (self.on_run)(change, window, cx);
        }
    }
}

impl Render for FormView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = Settings::get(cx).buffer_font_family.clone();
        let built = (self.build)(&self.values(cx));
        let caption = |t: SharedString| {
            div()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(t)
        };
        let mut checks = Vec::new();
        let mut rows = Vec::new();
        for (i, (_, label, c)) in self.fields.iter().enumerate() {
            match c {
                Control::Text(input) => rows.push(
                    v_flex()
                        .gap_1()
                        .child(caption(label.clone()))
                        .child(Input::new(input))
                        .into_any_element(),
                ),
                Control::Choice(sel) => rows.push(
                    v_flex().gap_1().child(caption(label.clone())).child(Select::new(sel)).into_any_element(),
                ),
                Control::Check(b) => checks.push(
                    div()
                        .w(px(250.))
                        .child(Switch::new(("form-check", i)).label(label.clone()).checked(*b).on_change(
                            cx.listener(move |this, v, _, cx| {
                                if let Some((_, _, Control::Check(c))) = this.fields.get_mut(i) {
                                    *c = *v;
                                }
                                cx.notify();
                            }),
                        ))
                        .into_any_element(),
                ),
            }
        }
        v_flex()
            .gap_3()
            .when_some(self.intro.clone(), |d, t| {
                d.child(div().text_size(px(12.)).text_color(theme.muted_foreground).child(t))
            })
            .children(rows)
            .when(!checks.is_empty(), |d| d.child(h_flex().flex_wrap().gap_x_3().gap_y_2().children(checks)))
            .child(
                v_flex()
                    .gap_1()
                    .child(caption("SQL that will run".into()))
                    .child(
                        div()
                            .id("form-preview")
                            .max_h(px(120.))
                            .overflow_y_scroll()
                            .p_2()
                            .rounded_md()
                            .bg(theme.muted)
                            .border_1()
                            .border_color(theme.border)
                            .font_family(mono)
                            .text_size(px(12.))
                            .child(match &built {
                                Ok(c) => format!("{};", c.shown),
                                Err(_) => "—".to_string(),
                            }),
                    )
                    .when_some(built.as_ref().err().cloned(), |d, e| {
                        d.child(div().text_size(px(12.)).text_color(theme.danger).child(e))
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("form-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
                    .child(
                        Button::new("form-run")
                            .primary()
                            .label(self.submit.clone())
                            .disabled(built.is_err())
                            .on_click(cx.listener(|this, _, w, cx| this.run(w, cx))),
                    ),
            )
    }
}
