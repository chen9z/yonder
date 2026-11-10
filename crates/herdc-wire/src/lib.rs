//! A herdr-core client that does not care how bytes travel: give it the two
//! halves of a pipe that runs `herdr-core connect` (over SSH, or a local
//! child for tests) and it speaks the framed protocol of herdr-core's
//! `schema/README.md`.

mod client;
pub mod compat;
mod events;
pub mod frame;

pub use client::{Attach, Attached, Client, Item, Stream, ATTACH_CREDIT, CALL_TIMEOUT, EVENT_CREDIT};
pub use events::{event_line, Pushed, Terminals};

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The core answered with an error: `kind` is stable (`not_found`,
    /// `conflict`, `unavailable`, `needs_login`, ...), `hint` says what to do.
    #[error("{method}: {message}{}", hint.as_ref().map(|h| format!(" ({h})")).unwrap_or_default())]
    Rpc { method: String, kind: Option<String>, message: String, hint: Option<String> },
    #[error("{0}: no reply from the core")]
    Timeout(String),
    #[error("lost the connection to the core")]
    Lost,
    #[error("{0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    fn rpc(method: &str, error: &Value) -> Self {
        Error::Rpc {
            method: method.to_owned(),
            kind: error.pointer("/data/kind").and_then(Value::as_str).map(str::to_owned),
            message: error.get("message").and_then(Value::as_str).unwrap_or("the core refused").to_owned(),
            hint: error.pointer("/data/hint").and_then(Value::as_str).filter(|h| !h.is_empty()).map(str::to_owned),
        }
    }

    /// The core's stable error kind, if it answered with one.
    pub fn kind(&self) -> Option<&str> {
        match self {
            Error::Rpc { kind, .. } => kind.as_deref(),
            _ => None,
        }
    }
}
