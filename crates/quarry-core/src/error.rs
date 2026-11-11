use serde::{Deserialize, Serialize};

/// A position inside the SQL text that the server blamed for an error.
/// `offset` is a **0-based byte offset** into the statement that was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPosition {
    pub offset: usize,
}

/// Every failure a driver can report. The UI matches on the variant to decide
/// presentation (e.g. `Query` errors are shown inline under the statement,
/// `Connect`/`Tunnel` errors in the connection dialog).
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
pub enum Error {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("SSH tunnel failed: {0}")]
    Tunnel(String),
    /// The server rejected a statement.
    #[error("{message}")]
    Query {
        message: String,
        /// SQLSTATE or vendor code, when the server gave one.
        code: Option<String>,
        detail: Option<String>,
        hint: Option<String>,
        position: Option<ErrorPosition>,
    },
    #[error("query cancelled")]
    Cancelled,
    #[error("connection closed")]
    Closed,
    #[error("not supported by this database: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn query(message: impl Into<String>) -> Self {
        Error::Query { message: message.into(), code: None, detail: None, hint: None, position: None }
    }
    pub fn other(e: impl std::fmt::Display) -> Self {
        Error::Other(e.to_string())
    }

    /// Whether the *session* is gone rather than the statement being wrong: the server was
    /// shut down or the backend terminated (`pg_terminate_backend`, `KILL`), the socket broke
    /// or hit EOF. Every further statement on this connection would fail the same way, so the
    /// host reconnects.
    ///
    /// Codes are matched exactly (a server may legitimately say "broken pipe" in the message
    /// of an unrelated error); free-form text is only consulted for errors that carry no code,
    /// which is how drivers report transport failures.
    pub fn is_connection_lost(&self) -> bool {
        match self {
            Error::Closed => true,
            Error::Query { code: Some(code), .. } => lost_code(code),
            Error::Query { message, code: None, .. } => lost_text(message),
            Error::Other(message) => lost_text(message),
            Error::Connect(_)
            | Error::Auth(_)
            | Error::Tunnel(_)
            | Error::Cancelled
            | Error::Unsupported(_) => false,
        }
    }
}

/// Server error codes that mean the session ended.
fn lost_code(code: &str) -> bool {
    // PostgreSQL: class 08 (connection exception) and the shutdown family of class 57
    // (admin_shutdown, crash_shutdown, cannot_connect_now). MySQL / MariaDB: 1053 server
    // shutdown in progress, 1927 connection was killed, 2006 server has gone away, 2013 lost
    // connection during query, 2055 lost connection at handshake, 4031 idle timeout.
    code.starts_with("08")
        || matches!(code, "57P01" | "57P02" | "57P03")
        || matches!(code, "1053" | "1927" | "2006" | "2013" | "2055" | "4031")
}

/// Transport-level failure texts (drivers' own wording, any case).
fn lost_text(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "server has gone away",
        "lost connection",
        "broken pipe",
        "connection reset",
        "connection aborted",
        "connection closed",
        "connection is closed",
        "connection was killed",
        "terminating connection",
        "unexpected eof",
        "unexpected end of file",
        "end of file",
    ]
    .iter()
    .any(|p| m.contains(p))
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    fn q(code: &str, message: &str) -> Error {
        Error::Query {
            message: message.into(),
            code: Some(code.into()),
            detail: None,
            hint: None,
            position: None,
        }
    }

    #[test]
    fn postgres_session_loss_codes() {
        for code in ["08000", "08003", "08006", "08001", "08004", "57P01", "57P02", "57P03"] {
            assert!(q(code, "x").is_connection_lost(), "{code}");
        }
        // Ordinary statement errors are not session loss.
        for code in ["42601", "23505", "42P01", "57014", "40001", "22012", "0A000"] {
            assert!(!q(code, "x").is_connection_lost(), "{code}");
        }
    }

    #[test]
    fn mysql_session_loss_codes() {
        for code in ["1053", "1927", "2006", "2013"] {
            assert!(q(code, "x").is_connection_lost(), "{code}");
        }
        // 1064 syntax error, 1062 duplicate key, 1317 query interrupted (cancel), 1205 lock wait.
        for code in ["1064", "1062", "1317", "1205"] {
            assert!(!q(code, "x").is_connection_lost(), "{code}");
        }
    }

    #[test]
    fn transport_failures_by_text_and_closed() {
        assert!(Error::Closed.is_connection_lost());
        for m in [
            "MySQL server has gone away",
            "Lost connection to MySQL server during query",
            "Broken pipe (os error 32)",
            "connection reset by peer",
            "unexpected EOF",
            "connection closed",
            "error communicating with the server: unexpected end of file",
        ] {
            assert!(Error::Other(m.into()).is_connection_lost(), "{m}");
        }
        assert!(!Error::Other("invalid input syntax".into()).is_connection_lost());
    }

    #[test]
    fn a_server_message_with_a_code_is_never_judged_by_its_text() {
        let e = q("42601", "syntax error near 'broken pipe'");
        assert!(!e.is_connection_lost());
        // Without a code (a driver-side failure) the text decides.
        assert!(Error::query("lost connection to server").is_connection_lost());
    }

    #[test]
    fn not_session_loss() {
        assert!(!Error::Cancelled.is_connection_lost());
        assert!(!Error::Connect("connection refused".into()).is_connection_lost());
        assert!(!Error::Auth("bad password".into()).is_connection_lost());
        assert!(!Error::Tunnel("x".into()).is_connection_lost());
        assert!(!Error::Unsupported("x".into()).is_connection_lost());
    }
}
