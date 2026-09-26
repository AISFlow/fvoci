//! Post-restore storage verification.
//!
//! A restored database is only usable if every stored attachment it
//! references exists in the configured storage with the recorded size. For
//! the local driver the objects come from the backup's storage archive; for
//! S3 they come from the operator's bucket (versioning or replication), which
//! the backup scripts do not copy. This check HEADs every stored key through
//! `ObjectStorage`, so it answers the same question for both drivers.
//!
//! A published image preview (`variants.preview`) is checked the same way
//! against its recorded byte size. The product never regenerates a published
//! preview (a new one is only published while `variants.preview` is absent),
//! so a missing preview object would fail every preview download: it fails
//! verification instead of being reported as regenerable.
//!
//! Branding assets (instance `branding.logo` / `branding.favicon`) live in the
//! same storage under their recorded key. They have no recorded size, so each
//! one is read back (at most `BRANDING_ASSET_MAX_BYTES`) and must hash to its
//! recorded SHA-256, the same check the public branding route makes before
//! serving it.

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::ObjectStorage;
use crate::db::attachments::{list_all_workspace_ids, list_workspace_stored_objects};
use crate::http::routes::admin::BRANDING_ASSET_MAX_BYTES;
use crate::settings::BrandingAssetKind;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageVerifyReport {
    pub checked: u64,
    /// Attachment ids whose object is missing.
    pub missing: Vec<Uuid>,
    /// Attachment ids whose object size differs from `size_bytes`.
    pub size_mismatch: Vec<Uuid>,
    /// Published previews checked.
    pub preview_checked: u64,
    /// Attachment ids whose published preview object is missing.
    pub preview_missing: Vec<Uuid>,
    /// Attachment ids whose preview object size differs from its recorded
    /// `bytes`.
    pub preview_size_mismatch: Vec<Uuid>,
    /// Branding assets referenced by the instance settings.
    pub branding_checked: u64,
    /// Branding asset kinds (`logo`, `favicon`) whose object is missing.
    pub branding_missing: Vec<&'static str>,
    /// Branding asset kinds whose object is empty, over the size limit or
    /// does not match the recorded SHA-256.
    pub branding_mismatch: Vec<&'static str>,
}

impl StorageVerifyReport {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
            && self.size_mismatch.is_empty()
            && self.preview_missing.is_empty()
            && self.preview_size_mismatch.is_empty()
            && self.branding_missing.is_empty()
            && self.branding_mismatch.is_empty()
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
            let Some(preview) = &object.preview else {
                continue;
            };
            report.preview_checked += 1;
            match storage.head(&preview.key).await {
                Ok(Some(size)) if size as i64 == preview.bytes => {}
                Ok(Some(_)) => report.preview_size_mismatch.push(object.id),
                Ok(None) => report.preview_missing.push(object.id),
                Err(err) => {
                    return Err(format!(
                        "storage check for the preview of attachment {}: {err}",
                        object.id
                    ));
                }
            }
        }
    }
    verify_branding_assets(pool, storage, &mut report).await?;
    Ok(report)
}

async fn verify_branding_assets(
    pool: &PgPool,
    storage: &ObjectStorage,
    report: &mut StorageVerifyReport,
) -> Result<(), String> {
    // Product reads may hide branding when the license is absent; restore checks
    // must still HEAD/hash every asset leaf stored in the settings row.
    let values = crate::settings::persisted_values(pool, "FVOCI")
        .await
        .map_err(|e| format!("read instance settings: {e}"))?;
    for kind in [BrandingAssetKind::Logo, BrandingAssetKind::Favicon] {
        let Some(asset) = values.branding.asset(kind) else {
            continue;
        };
        report.branding_checked += 1;
        let key = asset.key.to_string();
        let storage_error = |err: super::StorageError| {
            format!("storage check for branding {}: {err}", kind.as_str())
        };
        let size = match storage.head(&key).await.map_err(storage_error)? {
            None => {
                report.branding_missing.push(kind.as_str());
                continue;
            }
            Some(size) => size,
        };
        if size == 0 || size > BRANDING_ASSET_MAX_BYTES as u64 {
            report.branding_mismatch.push(kind.as_str());
            continue;
        }
        let bytes = storage
            .read_range(&key, 0, size - 1)
            .await
            .map_err(storage_error)?;
        if hex::encode(Sha256::digest(&bytes)) != asset.sha256 {
            report.branding_mismatch.push(kind.as_str());
        }
    }
    Ok(())
}
