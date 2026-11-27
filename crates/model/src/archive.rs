//! The native `.fvoci` archive format used for import and export.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The content of `version.json`, the first entry of an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveHeader {
    /// The archive format version.
    pub version: i64,
    /// The export time in milliseconds since the Unix epoch.
    pub date: i64,
}

/// One line of a container's `container.jsonl`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveLine {
    /// What `data` holds: `container`, `block`, `containerMember`, or `courseSchedule`.
    #[serde(rename = "type")]
    pub line_type: String,
    /// The entity itself, decoded according to `line_type`.
    pub data: Value,
}
