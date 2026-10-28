//! SDM core: an original segmented download engine.
//!
//! Splits a file into byte-range segments, fetches them over several parallel
//! HTTP(S) connections, writes each at its correct offset, steals work so idle
//! connections help finish the tail, and supports pause/resume via a small JSON
//! sidecar. No kernel hooks or traffic interception — just the HTTP `Range`
//! mechanism any client is entitled to use.

mod engine;
mod manager;
mod types;
mod util;

pub use engine::{Download, Segment, STATE_SUFFIX, WORKING_SUFFIX};
pub use manager::Manager;
pub use types::{SdmError, Status};
pub use util::{
    config_dir, default_download_dir, default_headers, filename_from_url, human_bytes, human_time,
    parse_content_disposition, percent_decode, sanitize_filename, DEFAULT_UA,
};
