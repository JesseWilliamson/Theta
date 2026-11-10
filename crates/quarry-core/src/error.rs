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
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
