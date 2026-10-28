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
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}
