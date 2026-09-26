use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use document_extract_client::limits::{Limits, MAX_INPUT_BYTES, MAX_WARNING_ENTRIES};

use crate::db::identity::{append_event, EventAppend};
use crate::db::search_index::replace_attachment_chunks;
use crate::search::chunk::chunk_plain_text;

pub const EXTRACT_LEASE_SECS: u64 = 300;
pub const EXTRACT_MAX_ATTEMPTS: i16 = 2;
pub const EXTRACT_RETRY_BACKOFF_MS: u64 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractClaim {
    pub workspace_id: Uuid,
    pub attachment_id: Uuid,
    pub lease_token: Uuid,
    pub attempt: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractInput {
    pub storage_key: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishExtract {
    pub status: String,
    pub text: String,
    pub warnings: Vec<String>,
    pub rhwp_rev: Option<String>,
}

pub async fn claim_extract(pool: &PgPool) -> Result<Option<ExtractClaim>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT workspace_id, attachment_id, lease_token, attempt
        FROM fvoci.app_claim_attachment_extract()
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| ExtractClaim {
        workspace_id: row.get("workspace_id"),
        attachment_id: row.get("attachment_id"),
        lease_token: row.get("lease_token"),
        attempt: row.get("attempt"),
    }))
}

pub async fn load_extract_input(
    pool: &PgPool,
    claim: &ExtractClaim,
) -> Result<Option<ExtractInput>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, claim.workspace_id).await?;
    let row = sqlx::query(
        r#"
        SELECT storage_key, name, mime, size_bytes
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row.map(|row| ExtractInput {
        storage_key: row.get("storage_key"),
        name: row.get("name"),
        mime: row.get("mime"),
        size_bytes: row.get("size_bytes"),
    }))
}

pub async fn finish_extract(
    pool: &PgPool,
    claim: &ExtractClaim,
    finish: &FinishExtract,
) -> Result<bool, sqlx::Error> {
    let warnings = bounded_warnings_json(&finish.warnings);
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, claim.workspace_id).await?;

    let parent: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT document_id, task_id
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(parent) = parent else {
        tx.rollback().await?;
        return Ok(false);
    };

    let workspace_live: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(claim.workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    if !workspace_live.map(|(live,)| live).unwrap_or(false) {
        tx.rollback().await?;
        return Ok(false);
    }

    let parent_live = match parent {
        (Some(document_id), _) => sqlx::query_scalar::<_, bool>(
            r#"
            SELECT deleted_at IS NULL
            FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
            FOR UPDATE
            "#,
        )
        .bind(claim.workspace_id)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false),
        (None, Some(task_id)) => sqlx::query_scalar::<_, bool>(
            r#"
            SELECT deleted_at IS NULL
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            FOR NO KEY UPDATE
            "#,
        )
        .bind(claim.workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false),
        (None, None) => false,
    };
    if !parent_live {
        tx.rollback().await?;
        return Ok(false);
    }

    let attachment_locked: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT document_id, task_id
        FROM fvoci.attachments
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        FOR UPDATE
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .fetch_optional(&mut *tx)
    .await?;
    if attachment_locked != Some(parent) {
        tx.rollback().await?;
        return Ok(false);
    }

    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_status = $4,
            extract_text = $5,
            extract_warnings = $6,
            extract_rhwp_rev = $7,
            extract_lease_token = NULL,
            extract_lease_expires_at = NULL
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .bind(&finish.status)
    .bind(&finish.text)
    .bind(warnings)
    .bind(&finish.rhwp_rev)
    .execute(&mut *tx)
    .await?;

    let chunks = if finish.status == "ok" || finish.status == "partial" {
        chunk_plain_text(&finish.text)
    } else {
        Vec::new()
    };
    replace_attachment_chunks(
        &mut tx,
        claim.workspace_id,
        claim.attachment_id,
        &finish.status,
        &chunks,
    )
    .await?;
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(claim.workspace_id),
            actor_user_id: None,
            verb: "attachment.extracted".into(),
            target_type: Some("attachment".into()),
            target_id: Some(claim.attachment_id),
            payload: json!({
                "attachmentId": claim.attachment_id.to_string(),
                "status": finish.status,
            }),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(updated.rows_affected() > 0)
}

pub async fn release_extract(pool: &PgPool, claim: &ExtractClaim) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, claim.workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_lease_token = NULL,
            extract_lease_expires_at = NULL,
            extract_attempts = GREATEST(extract_attempts - 1, 0)
        WHERE workspace_id = $1
          AND id = $2
          AND extract_lease_token = $3
          AND status = 'stored'
          AND extract_status = 'pending'
        "#,
    )
    .bind(claim.workspace_id)
    .bind(claim.attachment_id)
    .bind(claim.lease_token)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(updated.rows_affected() > 0)
}

pub fn default_extract_limits() -> Limits {
    Limits::default()
}

pub fn oversize_resource_limit(size_bytes: i64) -> FinishExtract {
    FinishExtract {
        status: "resource_limit".into(),
        text: String::new(),
        warnings: vec![format!(
            "size_bytes {} exceeds {}-byte extract input limit",
            size_bytes, MAX_INPUT_BYTES
        )],
        rhwp_rev: None,
    }
}

fn bounded_warnings_json(warnings: &[String]) -> Value {
    let capped = warnings
        .iter()
        .take(MAX_WARNING_ENTRIES)
        .collect::<Vec<_>>();
    json!(capped)
}

#[derive(Debug, Clone)]
pub struct ExtractRowState {
    pub extract_status: String,
    pub extract_text: String,
    pub extract_attempts: i16,
    pub lease_token: Option<Uuid>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub warnings: Value,
    pub rhwp_rev: Option<String>,
}

pub async fn fetch_extract_state(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<Option<ExtractRowState>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, workspace_id).await?;
    let row = sqlx::query(
        r#"
        SELECT extract_status,
               extract_text,
               extract_attempts,
               extract_lease_token,
               extract_lease_expires_at,
               extract_warnings,
               extract_rhwp_rev
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(row.map(|row| ExtractRowState {
        extract_status: row.get("extract_status"),
        extract_text: row.get("extract_text"),
        extract_attempts: row.get("extract_attempts"),
        lease_token: row.get("extract_lease_token"),
        lease_expires_at: row.get("extract_lease_expires_at"),
        warnings: row.get("extract_warnings"),
        rhwp_rev: row.get("extract_rhwp_rev"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_bounded() {
        let warnings = (0..40).map(|i| i.to_string()).collect::<Vec<_>>();
        let json = bounded_warnings_json(&warnings);
        assert_eq!(json.as_array().unwrap().len(), MAX_WARNING_ENTRIES);
    }
}
