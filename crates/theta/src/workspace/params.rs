//! Asking for query parameters before a run or an explain.

use super::Workspace;
use crate::dialogs::params as dialog;
use crate::exec::params::{apply, detect_picks, prefill};
use crate::exec::pick::Picked;
use crate::panels::messages::MessageKind;
use gpui_kit::{Context, Window};
use std::rc::Rc;
use theta_sql::params as p;
use theta_ui::Settings;

impl Workspace {
    /// Call `then` with `picks`, after asking for the values of any placeholders in them. With the
    /// setting off, or nothing to ask for, `then` runs at once with the statements unchanged.
    ///
    /// The values are remembered on the console (keyed by placeholder label) so the next run opens
    /// pre-filled, and the Messages log gets one entry listing what was substituted.
    pub(super) fn with_params(
        &mut self,
        console: u64,
        picks: Vec<Picked>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Vec<Picked>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !Settings::get(cx).editor.parameters {
            then(self, picks, window, cx);
            return;
        }
        let kind = self.console_kind(console, cx);
        let detected = detect_picks(&picks, kind);
        if detected.is_empty() {
            then(self, picks, window, cx);
            return;
        }
        let initial = self
            .consoles
            .get(&console)
            .map(|c| prefill(&detected.slots, &c.param_values))
            .unwrap_or_else(|| vec![p::ParamValue::default(); detected.slots.len()]);
        let note = if picks.len() > 1 {
            format!(
                "Values apply to all {}. They are inserted into the SQL as literals.",
                theta_core::text::plural(picks.len() as u64, "statement", "statements")
            )
        } else {
            "Values are inserted into the SQL as literals before it runs.".to_string()
        };
        let dialog_slots = detected.slots.clone();
        #[cfg(test)]
        if let Some(values) = self.param_answer.take() {
            self.finish_params(console, picks, detected, values, then, window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let then = std::cell::RefCell::new(Some(then));
        let submit: dialog::Submit = Rc::new(move |values, window, cx| {
            let Some(then) = then.borrow_mut().take() else { return };
            let (picks, detected) = (picks.clone(), detected.clone());
            ws.update(cx, |this, cx| this.finish_params(console, picks, detected, values, then, window, cx))
                .ok();
        });
        dialog::open(&dialog_slots, initial, kind, note, submit, window, cx);
    }

    /// The dialog was answered: substitute, remember, log and continue with the substituted statements.
    #[allow(clippy::too_many_arguments)]
    fn finish_params(
        &mut self,
        console: u64,
        picks: Vec<Picked>,
        detected: p::Detected,
        values: Vec<p::ParamValue>,
        then: impl FnOnce(&mut Self, Vec<Picked>, &mut Window, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = self.console_kind(console, cx);
        let substituted = match apply(&picks, &detected, &values, kind) {
            Ok(s) => s,
            Err(e) => {
                self.notify(crate::connections::NoticeKind::Error, format!("Parameters: {e}"), window, cx);
                return;
            }
        };
        if let Some(c) = self.consoles.get_mut(&console) {
            for (k, v) in detected.slots.iter().zip(&values) {
                c.param_values.insert(k.label(), v.clone());
            }
        }
        if let Some(conn) = self.tabs.iter().find(|t| t.id == console).and_then(|t| t.connection_id.clone()) {
            let first = substituted.first().map(|s| s.sql.clone()).unwrap_or_default();
            let mut lines = vec!["Parameters:".to_string()];
            lines.extend(p::describe(&detected.slots, &values).into_iter().map(|l| format!("  {l}")));
            self.log(&conn, &first, MessageKind::Info, lines, None, cx);
        }
        then(self, substituted, window, cx);
    }
}
