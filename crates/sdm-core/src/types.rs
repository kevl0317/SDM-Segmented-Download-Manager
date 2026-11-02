//! Shared status enum and the wording of everything that can go wrong.

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

/// A server's refusal, with a hint where there's an obvious one.
pub fn describe_status(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 | 403 => format!("{status}: the link may have expired or need a sign-in"),
        404 | 410 => format!("{status}: the file is no longer there"),
        _ => status.to_string(),
    }
}

/// A failed request in words a person can act on: the server's answer
/// ("401 Unauthorized") with a hint where there's an obvious one, and never the
/// URL, which can be a long signed link.
pub fn describe_http(e: &reqwest::Error) -> String {
    if let Some(status) = e.status() {
        return describe_status(status);
    }
    // Before the timeout check: a connection attempt that timed out is both.
    if e.is_connect() {
        return "Couldn't reach the server".to_string();
    }
    if e.is_timeout() {
        return TOO_SLOW.to_string();
    }
    if e.is_redirect() {
        return "The link redirects too many times".to_string();
    }
    if e.is_body() || e.is_decode() {
        return CONNECTION_LOST.to_string();
    }
    let text = e.to_string();
    text.split(" for url (").next().unwrap_or(&text).to_string()
}

pub(crate) const TOO_SLOW: &str = "The server took too long to answer";
pub(crate) const CONNECTION_LOST: &str = "The connection was lost";

/// A failed write or file operation, in words a person can act on.
pub fn describe_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match (e.raw_os_error(), e.kind()) {
        (Some(112), _) | (_, StorageFull) => "Not enough disk space".to_string(),
        // Windows answers a size NTFS can't hold with "The parameter is incorrect".
        (Some(87 | 223), _) | (_, FileTooLarge) => "The file is too big for this drive".to_string(),
        (_, PermissionDenied) => "Windows refused access to the file or folder".to_string(),
        (Some(3), _) | (_, NotFound) => "The file or folder can't be found".to_string(),
        (Some(32 | 33), _) => "Another program is using the file".to_string(),
        (Some(206), _) => "The file's name or path is too long".to_string(),
        _ => e.to_string(),
    }
}
