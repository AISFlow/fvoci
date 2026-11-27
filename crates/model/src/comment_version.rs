//! The earlier texts of a comment.

use serde::Serialize;

use crate::body::Body;

/// A text a comment had before its body was changed: the body then, who wrote it and
/// when. The first is the comment as it was posted.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommentVersion {
    /// The place of the version among the comment's, counted from 1 for the text it was
    /// posted with. Versions the server no longer keeps leave gaps.
    pub version: i64,
    /// The body of the comment then; `null` if it had none.
    pub body: Option<Body>,
    /// Who wrote this version: the author of the comment for the first one, and for
    /// each later one who changed the body to it.
    pub written_by: String,
    /// When this version was written, in milliseconds since the Unix epoch.
    pub written_at: i64,
}
