//! Shared status enum and error type for the download engine.

use serde::{Deserialize, Serialize};

/// Lifecycle of a single download, mirroring the original engine's states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Probing,
    Running,
    Paused,
    Complete,
    Error,
}

impl Status {
    /// The lowercase wire name, matching the JSON the Python build wrote.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Probing => "probing",
            Status::Running => "running",
            Status::Paused => "paused",
            Status::Complete => "complete",
            Status::Error => "error",
        }
    }

    /// Queued/probing/running — i.e. the orchestrator may still be working.
    pub fn is_active(self) -> bool {
        matches!(self, Status::Queued | Status::Probing | Status::Running)
    }
}

/// Everything the engine can fail with. Network and IO errors convert in.
#[derive(Debug, thiserror::Error)]
pub enum SdmError {
    #[error("{}", describe_http(.0))]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// A failed request in words a person can act on: the server's answer
/// ("401 Unauthorized") with a hint where there's an obvious one, and never the
/// URL, which can be a long signed link.
pub fn describe_http(e: &reqwest::Error) -> String {
    if let Some(status) = e.status() {
        return match status.as_u16() {
            401 | 403 => format!("{status}: the link may have expired or need a sign-in"),
            404 | 410 => format!("{status}: the file is no longer there"),
            _ => status.to_string(),
        };
    }
    if e.is_timeout() {
        return "The server took too long to answer".to_string();
    }
    if e.is_connect() {
        return "Couldn't reach the server".to_string();
    }
    let text = e.to_string();
    text.split(" for url (").next().unwrap_or(&text).to_string()
}
