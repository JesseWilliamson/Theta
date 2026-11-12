//! Transaction mode (auto-commit or manual) and isolation level of a connection's session.
//!
//! The saved default comes from the connection's settings (`ConnectionConfig::transaction`); the title-bar
//! chip and the palette change it for the current session only. The pure parts (which statements start a
//! transaction at which level) live in `theta_core::txn`.

use super::Workspace;
use super::run::ManualBegin;
use crate::connections::NoticeKind;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, Context, IntoElement, SharedString, Window};
use theta_core::Family;
use theta_core::txn::{Isolation, SqliteBegin, TransactionConfig, isolation_levels};
use theta_core::{ConnectionConfig, DatabaseKind};
use theta_ui::picker::{PickerDelegate, PickerItem};

/// How a transaction statement ends the transaction, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TxnEnd {
    Commit,
    Rollback,
}

/// Whether `sql` is a plain `COMMIT`/`END` or `ROLLBACK`/`ABORT`. `ROLLBACK TO SAVEPOINT` and
/// `COMMIT ... AND CHAIN` leave a transaction open, so they are not ends.
pub(super) fn txn_end(sql: &str) -> Option<TxnEnd> {
    let mut rest = sql.trim_start();
    // Leading comments are not part of the statement.
    loop {
        if let Some(r) = rest.strip_prefix("--") {
            rest = r.split_once('\n').map_or("", |(_, t)| t).trim_start();
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r.split_once("*/").map_or("", |(_, t)| t).trim_start();
        } else {
            break;
        }
    }
    let words: Vec<String> = rest
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
        .split_whitespace()
        .map(|w| w.to_ascii_uppercase())
        .collect();
    let filler = |w: &String| matches!(w.as_str(), "TRANSACTION" | "WORK" | "TRAN");
    let plain = words.iter().skip(1).all(filler);
    match words.first().map(String::as_str) {
        Some("COMMIT" | "END") if plain => Some(TxnEnd::Commit),
        Some("ROLLBACK" | "ABORT") if plain => Some(TxnEnd::Rollback),
        _ => None,
    }
}

/// The footer text of a result that a rollback made untrue.
pub(super) const ROLLED_BACK: &str = "Rolled back: these rows may no longer exist";

#[cfg(test)]
mod txn_end_tests {
    use super::*;

    #[test]
    fn recognises_plain_ends_only() {
        assert_eq!(txn_end("COMMIT"), Some(TxnEnd::Commit));
        assert_eq!(txn_end("commit;"), Some(TxnEnd::Commit));
        assert_eq!(txn_end("END TRANSACTION"), Some(TxnEnd::Commit));
        assert_eq!(txn_end("-- done\nROLLBACK WORK;"), Some(TxnEnd::Rollback));
        assert_eq!(txn_end("/* x */ abort"), Some(TxnEnd::Rollback));
        assert_eq!(txn_end("ROLLBACK TO SAVEPOINT a"), None);
        assert_eq!(txn_end("COMMIT AND CHAIN"), None);
        assert_eq!(txn_end("BEGIN"), None);
        assert_eq!(txn_end("select 1"), None);
        assert_eq!(txn_end(""), None);
    }
}

/// What a chip-menu item does to the workspace.
type MenuAction = Box<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

impl Workspace {
    /// Note that the open transaction of `conn_id` has written something (once is enough).
    pub(super) fn note_txn_write(&mut self, conn_id: &str) {
        self.txn_writes.entry(conn_id.to_string()).or_insert_with(std::time::Instant::now);
    }

    /// The transaction of `conn_id` has ended. After a rollback, results fetched since its first write
    /// may show rows that were never stored, so they are marked stale (footer: Rolled back, with a
    /// Refresh when the statement can be run again). After a commit there is nothing to undo.
    pub(super) fn txn_ended(&mut self, conn_id: &str, rolled_back: bool, cx: &mut Context<Self>) {
        let Some(since) = self.txn_writes.remove(conn_id) else { return };
        if !rolled_back {
            return;
        }
        let ids: Vec<u64> =
            self.tabs.iter().filter(|t| t.connection_id.as_deref() == Some(conn_id)).map(|t| t.id).collect();
        for id in ids {
            let Some(c) = self.consoles.get_mut(&id) else { continue };
            let grid = c.grid.clone();
            for item in c.results.iter_mut().filter(|i| i.produced >= since) {
                item.meta.stale = Some(ROLLED_BACK.to_string());
                let showing = grid.read(cx).result().is_some_and(|r| std::sync::Arc::ptr_eq(r, &item.result));
                if showing {
                    grid.update(cx, |g, cx| g.set_stale(Some(ROLLED_BACK.to_string()), cx));
                }
            }
        }
        cx.notify();
    }

    /// The mode in force for `conn_id`: a session override, else the connection's saved setting.
    pub(super) fn txn_config(&self, conn_id: &str, cx: &App) -> TransactionConfig {
        self.txn_prefs
            .get(conn_id)
            .copied()
            .or_else(|| self.store.read(cx).config(conn_id).map(|c| c.transaction))
            .unwrap_or_default()
    }

    /// `Some` when `conn_id` is in manual mode: what to start transactions with.
    pub(super) fn manual_begin(&self, conn_id: &str, cx: &App) -> Option<ManualBegin> {
        let cfg = self.txn_config(conn_id, cx);
        let c: &ConnectionConfig = self.store.read(cx).config(conn_id)?;
        cfg.manual
            .then(|| ManualBegin { kind: c.kind, statements: cfg.begin_statements(c.kind, c.read_only) })
    }

    /// The title-bar chip: the mode, and while a transaction is open `Manual · TXN open · Repeatable Read`.
    /// Its menu switches mode and isolation level for this session.
    pub(super) fn render_txn_chip(&self, open: bool, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let conn = self.active_connection_id(cx)?;
        let kind = self.store.read(cx).config(&conn)?.kind;
        if !self.store.read(cx).state(&conn).is_connected() {
            return None;
        }
        let cfg = self.txn_config(&conn, cx);
        let label = match (cfg.manual, open) {
            (_, false) => cfg.summary(kind),
            (true, true) => {
                let level = match kind.family() {
                    Family::Sqlite => cfg.sqlite_begin.label(),
                    _ => cfg.isolation.label(),
                };
                if kind.family() != Family::Sqlite && cfg.isolation == Isolation::ServerDefault {
                    "Manual · TXN open".to_string()
                } else {
                    format!("Manual · TXN open · {level}")
                }
            }
            // A transaction the user began by hand in auto-commit mode.
            (false, true) => "Auto-commit · TXN open".to_string(),
        };
        let ws = cx.entity().downgrade();
        let chip = Button::new("txn-chip")
            .small()
            .icon(Icon::new(if open { IconName::TriangleAlert } else { IconName::ChevronDown }))
            .label(label)
            .tooltip("Transaction mode and isolation level")
            .when(open, |b| b.warning().outline())
            .when(!open, |b| b.ghost())
            .dropdown_menu(move |menu, _, _| {
                let item = |label: &str, checked: bool, f: MenuAction| {
                    let ws = ws.clone();
                    PopupMenuItem::new(SharedString::from(label.to_string())).checked(checked).on_click(
                        move |_, window, cx| {
                            ws.update(cx, |this, cx| f(this, window, cx)).ok();
                        },
                    )
                };
                let mut menu = menu
                    .item(item(
                        "Auto-commit",
                        !cfg.manual,
                        Box::new(|w, win, cx| w.set_txn_manual(false, win, cx)),
                    ))
                    .item(item("Manual", cfg.manual, Box::new(|w, win, cx| w.set_txn_manual(true, win, cx))))
                    .separator();
                match kind.family() {
                    Family::Sqlite => {
                        for m in SqliteBegin::ALL {
                            menu = menu.item(item(
                                &format!("BEGIN {}", m.label().to_uppercase()),
                                cfg.sqlite_begin == m,
                                Box::new(move |w, win, cx| w.set_sqlite_begin(m, win, cx)),
                            ));
                        }
                    }
                    _ => {
                        for l in isolation_levels(kind) {
                            let l = *l;
                            menu = menu.item(item(
                                l.label(),
                                cfg.isolation == l,
                                Box::new(move |w, win, cx| w.set_isolation(l, win, cx)),
                            ));
                        }
                    }
                }
                menu
            });
        Some(chip.into_any_element())
    }

    fn change_txn_config(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut TransactionConfig, DatabaseKind),
    ) {
        let Some(conn) = self.active_connection_id(cx) else {
            self.notify(NoticeKind::Info, "Choose a connection first", window, cx);
            return;
        };
        let Some(kind) = self.store.read(cx).config(&conn).map(|c| c.kind) else { return };
        let before = self.txn_config(&conn, cx);
        let mut next = before;
        edit(&mut next, kind);
        // Leaving manual mode with work in flight would have to commit or discard it silently;
        // make the user choose instead.
        if before.manual && !next.manual && self.txn.contains(&conn) {
            self.notify(
                NoticeKind::Info,
                "Commit or roll back the open transaction before switching to auto-commit",
                window,
                cx,
            );
            return;
        }
        if next == before {
            return;
        }
        self.txn_prefs.insert(conn.clone(), next);
        let note = if before.manual && self.txn.contains(&conn) && before != next {
            " (applies to the next transaction)"
        } else {
            ""
        };
        self.notify(NoticeKind::Info, format!("Transactions: {}{note}", next.summary(kind)), window, cx);
        cx.notify();
    }

    pub(super) fn set_txn_manual(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.change_txn_config(window, cx, |c, _| c.manual = manual);
    }

    pub(super) fn set_isolation(&mut self, level: Isolation, window: &mut Window, cx: &mut Context<Self>) {
        self.change_txn_config(window, cx, |c, _| c.isolation = level);
    }

    pub(super) fn set_sqlite_begin(
        &mut self,
        mode: SqliteBegin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_txn_config(window, cx, |c, _| c.sqlite_begin = mode);
    }

    /// The palette's *Transaction Isolation…* picker (or the SQLite begin modes).
    pub(super) fn open_isolation_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conn) = self.active_connection_id(cx) else {
            self.notify(NoticeKind::Info, "Choose a connection first", window, cx);
            return;
        };
        let Some(kind) = self.store.read(cx).config(&conn).map(|c| c.kind) else { return };
        let delegate =
            IsolationPicker { workspace: cx.entity().downgrade(), kind, current: self.txn_config(&conn, cx) };
        self.open_picker(delegate, window, cx);
    }
}

struct IsolationPicker {
    workspace: gpui_kit::WeakEntity<Workspace>,
    kind: DatabaseKind,
    current: TransactionConfig,
}

impl PickerDelegate for IsolationPicker {
    fn placeholder(&self) -> SharedString {
        match self.kind.family() {
            Family::Sqlite => "SQLite transaction mode…".into(),
            _ => "Transaction isolation level…".into(),
        }
    }
    fn items(&mut self, _: &mut App) -> Vec<PickerItem> {
        match self.kind.family() {
            Family::Sqlite => SqliteBegin::ALL
                .into_iter()
                .map(|m| {
                    PickerItem::new(m.label(), format!("BEGIN {}", m.label().to_uppercase()))
                        .detail(match m {
                            SqliteBegin::Deferred => "take locks when first needed",
                            SqliteBegin::Immediate => "take the write lock now",
                            SqliteBegin::Exclusive => "also block readers",
                        })
                        .checked(self.current.sqlite_begin == m)
                })
                .collect(),
            _ => isolation_levels(self.kind)
                .iter()
                .map(|l| PickerItem::new(l.label(), l.label()).checked(self.current.isolation == *l))
                .collect(),
        }
    }
    fn initial_selection(&self, items: &[PickerItem]) -> Option<usize> {
        items.iter().position(|i| i.checked)
    }
    fn confirm(&mut self, item: &PickerItem, window: &mut Window, cx: &mut App) {
        let id = item.id.to_string();
        let kind = self.kind;
        let _ = self.workspace.update(cx, |ws, cx| match kind.family() {
            Family::Sqlite => {
                if let Some(m) = SqliteBegin::ALL.into_iter().find(|m| m.label() == id) {
                    ws.set_sqlite_begin(m, window, cx);
                }
            }
            _ => {
                if let Some(l) = isolation_levels(kind).iter().find(|l| l.label() == id) {
                    ws.set_isolation(*l, window, cx);
                }
            }
        });
    }
}
