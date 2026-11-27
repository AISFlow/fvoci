//! Who has a file open in the editor: one person at a time.

use serde::{Deserialize, Serialize};

/// Whether somebody has a file open in the editor now, and who.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileLease {
    /// Whether somebody holds the file's lease: has it open in the editor.
    pub editing: bool,
    /// Who holds it, where the caller may see who that is (the caller included); empty
    /// for nobody, and for somebody the caller does not see.
    pub editor_id: String,
}

/// The lease a caller asks to hold: one held already, to be held on, or none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileLeaseRequest {
    /// The lease the caller holds, as `holdFileLease` gave it; empty to take one.
    pub lease_id: String,
}

/// A lease the caller holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeldFileLease {
    /// The lease, to hold on to it and to let it go with.
    pub lease_id: String,
    /// How many seconds it lasts from now unless held on to again.
    pub seconds: u32,
}
