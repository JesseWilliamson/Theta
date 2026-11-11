//! Server administration tabs: Sessions and Roles.
//!
//! Both are workspace tabs (`ConsoleTab::admin`) that own a session of their own to the server. The pure part
//! (the SQL per product, row mapping, statement generation, password masking) is `theta_sql::admin`; this
//! module is the views, their key map (`keys`) and the form dialog the Roles tab uses. They never touch the
//! workspace: they emit [`AdminEvent`]s, which `workspace/admin_tabs.rs` turns into Messages entries, notices
//! and consoles.

pub mod form;
pub mod keys;
pub mod roles;
pub mod sessions;

use crate::connections::{ConnectionStore, NoticeKind};
use crate::panels::messages::MessageKind;
use crate::runtime;
use gpui_kit::{App, Entity};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use theta_core::{Connection, Error};

/// What a tab asks of the workspace.
#[derive(Debug, Clone, PartialEq)]
pub enum AdminEvent {
    /// Add a Messages entry. `title` is what the statement column shows: it must already be safe to display
    /// (a password replaced by `'***'`).
    Log {
        connection_id: String,
        title: String,
        kind: MessageKind,
        lines: Vec<String>,
    },
    Notice(NoticeKind, String),
    OpenInConsole {
        connection_id: String,
        sql: String,
    },
    /// Counts or titles changed (the tab strip may need to repaint).
    StateChanged,
}

/// Refresh intervals offered in the Sessions tab: label and milliseconds.
pub const INTERVALS: [(&str, u64); 6] =
    [("1 s", 1000), ("2 s", 2000), ("5 s", 5000), ("10 s", 10_000), ("30 s", 30_000), ("60 s", 60_000)];

type Opening = Pin<Box<dyn Future<Output = Result<Arc<dyn Connection>, Error>> + Send>>;

/// The tab's own session. It is lent to a background task (`take_or_open`) and handed back (`put`), so two
/// statements never run on it at once; if it is out when another is needed, a second one is opened.
#[derive(Default)]
pub struct SideConnection {
    conn: Option<Arc<dyn Connection>>,
}

impl SideConnection {
    pub fn take_or_open(&mut self, store: &Entity<ConnectionStore>, id: &str, cx: &App) -> Opening {
        if let Some(c) = self.conn.take() {
            return Box::pin(async move { Ok(c) });
        }
        match store.read(cx).open_side_connection(id) {
            Some(f) => Box::pin(f),
            None => Box::pin(async { Err(Error::Other("the connection no longer exists".into())) }),
        }
    }

    pub fn put(&mut self, c: Arc<dyn Connection>) {
        if let Some(old) = self.conn.replace(c) {
            Self::close_in_background(old);
        }
    }

    pub fn discard(&mut self) {
        if let Some(old) = self.conn.take() {
            Self::close_in_background(old);
        }
    }

    pub fn close(&mut self) {
        self.discard();
    }

    fn close_in_background(c: Arc<dyn Connection>) {
        runtime::spawn(async move {
            let _ = c.close().await;
        });
    }
}

/// Which admin tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminKind {
    Sessions,
    Roles,
}

impl AdminKind {
    pub fn feature(self) -> theta_core::Feature {
        match self {
            AdminKind::Sessions => theta_core::Feature::Sessions,
            AdminKind::Roles => theta_core::Feature::Roles,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AdminKind::Sessions => "Sessions",
            AdminKind::Roles => "Roles",
        }
    }
}
