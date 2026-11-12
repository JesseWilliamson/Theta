//! One toast on screen at a time.
//!
//! The toast component stacks newer toasts over older ones with only a sliver of the old card showing,
//! so a "Connected to …" toast arriving with a start-up warning hid the warning and cut its text off.
//! Toasts therefore wait in a queue and the next one appears when the previous one closes. Warnings and
//! errors go ahead of information and success, and an identical message already waiting is not queued
//! twice. Everything a toast says that matters is also recorded in Messages by the caller.

use crate::connections::NoticeKind;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Global, SharedString, Window};
use std::collections::VecDeque;

const MAX_WAITING: usize = 12;

#[derive(Default)]
struct ToastQueue {
    active: bool,
    waiting: VecDeque<(NoticeKind, SharedString)>,
}

impl Global for ToastQueue {}

impl ToastQueue {
    /// `Some` when the toast can be shown now; otherwise it is queued.
    fn offer(&mut self, kind: NoticeKind, message: SharedString) -> Option<(NoticeKind, SharedString)> {
        if !self.active {
            self.active = true;
            return Some((kind, message));
        }
        if self.waiting.iter().any(|(k, m)| *k == kind && *m == message) {
            return None;
        }
        let urgent = matches!(kind, NoticeKind::Warning | NoticeKind::Error);
        if self.waiting.len() >= MAX_WAITING {
            // Keep the newest of the same urgency: drop the oldest calm one first.
            if let Some(i) =
                self.waiting.iter().position(|(k, _)| !matches!(k, NoticeKind::Warning | NoticeKind::Error))
            {
                self.waiting.remove(i);
            } else {
                self.waiting.pop_front();
            }
        }
        if urgent {
            let at = self
                .waiting
                .iter()
                .position(|(k, _)| !matches!(k, NoticeKind::Warning | NoticeKind::Error))
                .unwrap_or(self.waiting.len());
            self.waiting.insert(at, (kind, message));
        } else {
            self.waiting.push_back((kind, message));
        }
        None
    }

    /// The toast that was showing has closed.
    fn closed(&mut self) -> Option<(NoticeKind, SharedString)> {
        self.active = false;
        let next = self.waiting.pop_front()?;
        self.active = true;
        Some(next)
    }
}

pub fn show(kind: NoticeKind, message: SharedString, window: &mut Window, cx: &mut App) {
    if let Some((kind, message)) = cx.default_global::<ToastQueue>().offer(kind, message) {
        push(kind, message, window, cx);
    }
}

fn push(kind: NoticeKind, message: SharedString, window: &mut Window, cx: &mut App) {
    let note = match kind {
        NoticeKind::Info => Notification::info(message),
        NoticeKind::Success => Notification::success(message),
        NoticeKind::Warning => Notification::warning(message),
        NoticeKind::Error => Notification::error(message),
    }
    .on_close(|window, cx| {
        // Deferred: this runs inside the closing toast's own update.
        window.defer(cx, |window, cx| {
            if let Some((kind, message)) = cx.default_global::<ToastQueue>().closed() {
                push(kind, message, window, cx);
            }
        });
    });
    window.push_notification(note, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(s: &str) -> SharedString {
        s.to_string().into()
    }

    #[test]
    fn the_second_toast_waits_for_the_first() {
        let mut q = ToastQueue::default();
        assert!(q.offer(NoticeKind::Success, msg("Connected")).is_some());
        assert!(q.offer(NoticeKind::Warning, msg("Theme broken")).is_none());
        let next = q.closed().unwrap();
        assert_eq!(next.1, "Theme broken");
        assert!(q.closed().is_none());
        assert!(!q.active);
    }

    #[test]
    fn warnings_go_ahead_of_calm_toasts_and_duplicates_collapse() {
        let mut q = ToastQueue::default();
        q.offer(NoticeKind::Info, msg("first"));
        q.offer(NoticeKind::Success, msg("calm"));
        q.offer(NoticeKind::Error, msg("bad"));
        q.offer(NoticeKind::Warning, msg("warn"));
        q.offer(NoticeKind::Warning, msg("warn"));
        let order: Vec<String> = std::iter::from_fn(|| q.closed()).map(|(_, m)| m.to_string()).collect();
        assert_eq!(order, ["bad", "warn", "calm"]);
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut q = ToastQueue::default();
        q.offer(NoticeKind::Info, msg("shown"));
        for i in 0..40 {
            q.offer(NoticeKind::Info, msg(&format!("n{i}")));
        }
        assert_eq!(q.waiting.len(), MAX_WAITING);
        assert_eq!(q.waiting.back().unwrap().1, "n39");
    }
}
