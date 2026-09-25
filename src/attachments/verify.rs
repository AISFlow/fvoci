//! Post-restore storage verification.
//!
//! A restored database is only usable if every stored attachment it
//! references exists in the configured storage with the recorded size. For
//! the local driver the objects come from the backup's storage archive; for
//! S3 they come from the operator's bucket (versioning or replication), which
//! the backup scripts do not copy. This check HEADs every stored key through
//! `ObjectStorage`, so it answers the same question for both drivers.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::ObjectStorage;
use crate::db::attachments::{list_all_workspace_ids, list_workspace_stored_objects};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageVerifyReport {
    pub checked: u64,
    /// Attachment ids whose object is missing.
    pub missing: Vec<Uuid>,
    /// Attachment ids whose object size differs from `size_bytes`.
    pub size_mismatch: Vec<Uuid>,
}

impl StorageVerifyReport {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.size_mismatch.is_empty()
    }
}

/// Checks every stored attachment. A storage error (credentials, wrong
/// bucket, network) aborts the check instead of being counted as missing.
pub async fn verify_stored_objects(
    pool: &PgPool,
    storage: &ObjectStorage,
) -> Result<StorageVerifyReport, String> {
    let mut report = StorageVerifyReport::default();
    let workspaces = list_all_workspace_ids(pool)
        .await
        .map_err(|e| format!("list workspaces: {e}"))?;
    for workspace_id in workspaces {
        let objects = list_workspace_stored_objects(pool, workspace_id)
            .await
            .map_err(|e| format!("list stored attachments: {e}"))?;
        for object in objects {
            report.checked += 1;
            match storage.head(&object.storage_key).await {
                Ok(Some(size)) if size as i64 == object.size_bytes => {}
                Ok(Some(_)) => report.size_mismatch.push(object.id),
                Ok(None) => report.missing.push(object.id),
                Err(err) => {
                    return Err(format!("storage check for attachment {}: {err}", object.id));
                }
            }
        }
    }
    Ok(report)
}
