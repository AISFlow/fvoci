//! The versions kept of a file.

use serde::{Deserialize, Serialize};

/// A version kept of a file: the bytes the file had, when they were written and by
/// whom.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    /// The place of the version among the file's, counted from 1. The server keeps the
    /// newest ones: the lowest numbers go as new versions come.
    pub version: i64,
    /// When the version was written, in milliseconds since the Unix epoch: for the
    /// first, when the file was uploaded.
    pub created_at: i64,
    /// Who wrote the version; empty where nobody is known to have (the first version
    /// of a file from before versions were kept, and of a copy).
    pub created_by: String,
    /// The size of the version in bytes.
    pub size: i64,
    /// Whether the version is the file's newest: what `getFile` gives.
    pub is_current: bool,
    /// The version whose bytes this one took, for one made by `restoreFileVersion`;
    /// `null` for any other.
    pub restored_from: Option<i64>,
}

/// An uploaded file that is to become a file's new version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewFileVersion {
    /// The number of the version the new one was made from: the file's newest, as
    /// `getFileVersions` gave it when the bytes were read.
    pub base_version: i64,
    /// The identifier of the uploaded file whose bytes become the version, as
    /// `uploadFile` or `completeUpload` answered it.
    pub file_id: String,
}

/// From which version a file is taken back to an older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileVersionBase {
    /// The number of the file's newest version, as `getFileVersions` gave it.
    pub base_version: i64,
}
