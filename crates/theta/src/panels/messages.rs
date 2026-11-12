//! Messages tab: a scrolling log of what ran and what the server said.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    ClipboardItem, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use theta_ui::{CHROME_TEXT_SIZE, EmptyState, tool_button};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Ok,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub time: String,
    pub connection: String,
    pub statement: String,
    pub kind: MessageKind,
    /// Outcome line, then notices / error text.
    pub lines: Vec<String>,
    pub code: Option<String>,
    pub detail: Option<String>,
    pub hint: Option<String>,
}

impl Message {
    pub fn plain_text(&self) -> String {
        let mut s = format!("[{}] {} · {}\n", self.time, self.connection, self.statement);
        for l in &self.lines {
            s += &format!("  {l}\n");
        }
        if let Some(c) = &self.code {
            s += &format!("  {c}\n");
        }
        if let Some(d) = &self.detail {
            s += &format!("  detail: {d}\n");
        }
        if let Some(h) = &self.hint {
            s += &format!("  hint: {h}\n");
        }
        s
    }
}

/// `HH:MM:SS` in UTC-free local-ish form (seconds since midnight UTC; good enough for a log).
pub fn clock_now() -> String {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let offset = local_offset_secs();
    let t = (secs as i64 + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
}

fn local_offset_secs() -> i64 {
    // TZ handling without a dependency: honour a fixed offset in THETA_TZ_OFFSET_MIN, else UTC.
    std::env::var("THETA_TZ_OFFSET_MIN").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0) * 60
}

pub struct MessagesPanel {
    entries: Vec<Message>,
    scroll: ScrollHandle,
    stick: bool,
}

impl MessagesPanel {
    pub fn new() -> Self {
        Self { entries: vec![], scroll: ScrollHandle::new(), stick: false }
    }

    pub fn push(&mut self, m: Message, cx: &mut Context<Self>) {
        self.entries.push(m);
        if self.entries.len() > 2000 {
            self.entries.remove(0);
        }
        self.stick = true;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        cx.notify();
    }

    pub fn entries(&self) -> &[Message] {
        &self.entries
    }
}

impl Default for MessagesPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for MessagesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        if self.stick {
            self.stick = false;
            self.scroll.scroll_to_bottom();
        }
        if self.entries.is_empty() {
            return EmptyState::new(IconName::MessageSquare, "No messages")
                .description("Notices, warnings and statement outcomes are listed here.")
                .into_any_element();
        }
        let all_text = self.entries.iter().map(Message::plain_text).collect::<Vec<_>>().join("\n");
        let rows = self.entries.iter().enumerate().map(|(i, m)| {
            let color = match m.kind {
                MessageKind::Error => theme.danger,
                MessageKind::Ok => theme.foreground,
                MessageKind::Info => theme.muted_foreground,
                MessageKind::Warning => theme.warning,
            };
            let text = m.plain_text();
            v_flex()
                .id(("msg", i))
                .px_3()
                .py_1p5()
                .gap_0p5()
                .border_b_1()
                .border_color(theme.border.opacity(0.5))
                .hover(|s| s.bg(theme.list_hover))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }))
                .child(
                    h_flex()
                        .gap_2()
                        .text_size(CHROME_TEXT_SIZE)
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(m.time.clone()))
                        .child(SharedString::from(m.connection.clone()))
                        .child(
                            div()
                                .font_family(theta_ui::Settings::get(cx).buffer_font_family.clone())
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .child(SharedString::from(m.statement.clone())),
                        ),
                )
                .children(
                    m.lines.iter().map(|l| {
                        div().text_size(px(13.)).text_color(color).child(SharedString::from(l.clone()))
                    }),
                )
                .children(m.code.iter().filter(|c| shows_code(c)).map(|c| {
                    div().text_size(CHROME_TEXT_SIZE).text_color(theme.muted_foreground).child(c.clone())
                }))
                .children(m.detail.iter().map(|d| {
                    div()
                        .text_size(CHROME_TEXT_SIZE)
                        .text_color(theme.muted_foreground)
                        .child(format!("Detail: {d}"))
                }))
                .children(m.hint.iter().map(|h| {
                    div().text_size(CHROME_TEXT_SIZE).text_color(theme.warning).child(format!("Hint: {h}"))
                }))
        });
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h(px(26.))
                    .flex_shrink_0()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .justify_end()
                    .bg(theme.tab_bar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(tool_button("messages-copy", IconName::Copy, "Copy All").on_click(
                        move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(all_text.clone())),
                    ))
                    .child(
                        tool_button("messages-clear", IconName::Eraser, "Clear")
                            .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                    ),
            )
            .child(
                div()
                    .id("messages-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(v_flex().children(rows)),
            )
            .into_any_element()
    }
}

/// One way to say which error the server reported, whatever the engine: PostgreSQL gives the SQLSTATE as
/// its code, MySQL and MariaDB a vendor number plus a `SQLSTATE x` detail. Returns the muted line
/// ("SQLSTATE 42P01", "Error 1305 · SQLSTATE 42000") and the server's own DETAIL, if it has one beyond that.
pub fn uniform_code(code: Option<&str>, detail: Option<&str>) -> (Option<String>, Option<String>) {
    let detail = detail.map(str::trim).filter(|d| !d.is_empty());
    let code = code.map(str::trim).filter(|c| shows_code(c));
    match (code, detail) {
        (Some(c), Some(d)) if d.starts_with("SQLSTATE ") && c.chars().all(|ch| ch.is_ascii_digit()) => {
            (Some(format!("Error {c} · {d}")), None)
        }
        (Some(c), d) if c.len() == 5 && c.chars().all(|ch| ch.is_ascii_digit() || ch.is_ascii_uppercase()) => {
            (Some(format!("SQLSTATE {c}")), d.map(str::to_string))
        }
        (Some(c), d) => (Some(c.to_string()), d.map(str::to_string)),
        (None, d) => (None, d.map(str::to_string)),
    }
}

/// Drivers without a meaningful error code leave it out or say "Unknown".
fn shows_code(code: &str) -> bool {
    let c = code.trim();
    !c.is_empty() && !c.eq_ignore_ascii_case("unknown")
}

#[cfg(test)]
mod tests {
    use super::shows_code;

    #[test]
    fn meaningless_codes_are_hidden() {
        assert!(!shows_code("Unknown"));
        assert!(!shows_code(" "));
        assert!(shows_code("42601"));
        assert!(shows_code("ConstraintViolation"));
    }

    #[test]
    fn codes_read_the_same_for_every_engine() {
        use super::uniform_code;
        assert_eq!(uniform_code(Some("42P01"), None), (Some("SQLSTATE 42P01".into()), None));
        assert_eq!(
            uniform_code(Some("1305"), Some("SQLSTATE 42000")),
            (Some("Error 1305 · SQLSTATE 42000".into()), None)
        );
        assert_eq!(
            uniform_code(Some("23505"), Some("Key (id)=(1) already exists.")),
            (Some("SQLSTATE 23505".into()), Some("Key (id)=(1) already exists.".into()))
        );
        assert_eq!(uniform_code(Some("Unknown"), None), (None, None));
        assert_eq!(uniform_code(Some("ConstraintViolation"), None).0.as_deref(), Some("ConstraintViolation"));
    }
}
