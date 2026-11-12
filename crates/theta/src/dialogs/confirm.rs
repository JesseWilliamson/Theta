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

/// One statement of the SQL shown in the dialog, with the small labels that say what it does to the
/// database (`destructive`, `table rewrite`...).
#[derive(Clone, Default)]
pub struct Item {
    pub sql: String,
    /// `(text, is_danger)`: danger labels are drawn in the danger colour, the others as warnings.
    pub badges: Vec<(String, bool)>,
    /// A line of explanation under the badges.
    pub note: Option<String>,
}

impl Item {
    pub fn plain(sql: impl Into<String>) -> Self {
        Self { sql: sql.into(), ..Default::default() }
    }
}

struct ConfirmView {
    intro: SharedString,
    statements: Vec<Item>,
    choices: Vec<Choice>,
    /// One handle per button, Cancel first. The buttons hold the keyboard (Tab and the arrow keys walk
    /// them, Enter and Space press the focused one), so what Enter does is always the button that is
    /// drawn with a focus ring.
    buttons: Vec<gpui_kit::FocusHandle>,
    /// The button focused when the dialog opens (see [`default_button`]).
    initial: usize,
}

/// Which button has the keyboard when the dialog opens. Anything that destroys data defaults to
/// Cancel, so Enter pressed by reflex never destroys; otherwise it is the primary choice, then the
/// first one, so that "Enter" confirms a harmless question.
fn default_button(choices: &[Choice]) -> usize {
    if choices.iter().any(|c| c.tone == Tone::Danger) {
        return 0;
    }
    choices.iter().position(|c| c.tone == Tone::Primary).map_or(0, |i| i + 1)
}

impl ConfirmView {
    fn focused_button(&self, window: &Window) -> usize {
        self.buttons.iter().position(|h| h.is_focused(window)).unwrap_or(self.initial)
    }

    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.buttons.len() as isize;
        let next = (self.focused_button(window) as isize + delta).rem_euclid(n) as usize;
        window.focus(&self.buttons[next], cx);
    }

    /// Press button `ix`: Cancel closes, a choice closes and then runs.
    fn press(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        if let Some(choice) = ix.checked_sub(1).and_then(|i| self.choices.get(i)) {
            (choice.run.clone())(window, cx);
        }
    }
}

impl Render for ConfirmView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mono = theta_ui::Settings::get(cx).buffer_font_family.clone();
        v_flex()
            .gap_3()
            .on_key_down(cx.listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                let m = &ev.keystroke.modifiers;
                if m.control || m.alt || m.platform || m.shift {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "left" => this.move_focus(-1, window, cx),
                    "right" => this.move_focus(1, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
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
                        .child(v_flex().gap_1p5().children(self.statements.iter().map(|s| {
                            v_flex()
                                .gap_0p5()
                                .when(!s.badges.is_empty(), |d| {
                                    d.child(h_flex().gap_1().children(s.badges.iter().map(
                                        |(text, danger)| {
                                            let color = if *danger { theme.danger } else { theme.warning };
                                            div()
                                                .px_1p5()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(color.opacity(0.6))
                                                .bg(color.opacity(0.12))
                                                .text_size(px(10.))
                                                .text_color(color)
                                                .child(SharedString::from(text.clone()))
                                        },
                                    )))
                                })
                                .when_some(s.note.clone(), |d, note| {
                                    d.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.muted_foreground)
                                            .child(SharedString::from(format!("-- {note}"))),
                                    )
                                })
                                .child(div().child(SharedString::from(format!("{};", s.sql))))
                        }))),
                )
            })
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted_foreground)
                            .child("Tab or arrows switch button · Enter presses it · Esc cancels"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("confirm-cancel")
                                    .label("Cancel")
                                    .track_focus(&self.buttons[0])
                                    .on_click(cx.listener(|this, _, window, cx| this.press(0, window, cx))),
                            )
                            .children(self.choices.iter().enumerate().map(|(i, c)| {
                                let b = Button::new(("confirm-choice", i))
                                    .label(SharedString::from(c.label.clone()))
                                    .track_focus(&self.buttons[i + 1]);
                                let b = match c.tone {
                                    Tone::Primary => b.primary(),
                                    Tone::Danger => b.danger(),
                                    Tone::Plain => b,
                                };
                                b.on_click(cx.listener(move |this, _, window, cx| this.press(i + 1, window, cx)))
                            })),
                    ),
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
    open_items(window, cx, title, intro, statements.into_iter().map(Item::plain).collect(), choices)
}

/// Like [`open`], with a label row and a note per statement.
pub fn open_items(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    intro: impl Into<SharedString>,
    statements: Vec<Item>,
    choices: Vec<Choice>,
) {
    let initial = default_button(&choices);
    let view = cx.new(|cx| {
        let buttons = (0..=choices.len()).map(|_| cx.focus_handle()).collect();
        ConfirmView { intro: intro.into(), statements, choices, buttons, initial }
    });
    let first = view.read(cx).buttons[initial].clone();
    let title: SharedString = title.into();
    window.defer(cx, move |window, cx| window.focus(&first, cx));
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = view.clone();
        // Enter arrives here as the dialog's `Confirm` action, whichever button has the focus; it presses
        // the focused one (a bare "OK" would run the default choice even with Cancel focused).
        let on_ok = view.clone();
        dialog.bg(cx.theme().popover).title(title.clone()).w(px(640.)).child(view).on_ok(
            move |_, window, cx| {
                on_ok.update(cx, |v, cx| {
                    let ix = v.focused_button(window);
                    v.press(ix, window, cx);
                });
                false
            },
        )
    });
}
