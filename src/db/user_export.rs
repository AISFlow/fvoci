//! Data for GET /me/export (source packages/core/src/user-export.ts).
//!
//! Comments are paged per membership workspace under its tenant context so a
//! large history never sits in memory at once. Attachments come from the
//! uploader across workspaces (source `listStoredByUploader` in a system tx).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{clear_self_user, set_self_user, set_tenant};

pub const EXPORT_COMMENT_PAGE: i64 = 500;

pub struct ExportProfile {
    pub id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub locale: String,
    pub timezone: String,
    pub week_starts_on: i32,
}

pub struct ExportComment {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub struct ExportAttachment {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub mime: String,
    pub size_bytes: Option<i64>,
    pub scan_status: String,
    pub storage_key: String,
}

pub async fn export_profile(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<ExportProfile>, sqlx::Error> {
    type Row = (Uuid, String, String, Option<String>, String, String, i32);
    let row: Option<Row> = sqlx::query_as(
        r#"
            SELECT id, email, given_name, family_name, locale, timezone, week_starts_on
            FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL
            "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, email, given_name, family_name, locale, timezone, week_starts_on)| ExportProfile {
            id,
            email,
            given_name,
            family_name,
            locale,
            timezone,
            week_starts_on,
        },
    ))
}

pub async fn export_membership_workspaces(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, user_id).await?;
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT workspace_id FROM fvoci.memberships WHERE user_id = $1 ORDER BY created_at ASC, workspace_id ASC",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    clear_self_user(&mut tx).await?;
    tx.commit().await?;
    Ok(rows)
}

/// One keyset page of the user's comments in a workspace, (created_at, id) order.
pub async fn export_comment_page(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
) -> Result<Vec<ExportComment>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<Uuid>, String, DateTime<Utc>)>(
        r#"
        SELECT id, document_id, task_id, body, created_at
        FROM fvoci.comments
        WHERE workspace_id = $1 AND created_by = $2
          AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4))
        ORDER BY created_at ASC, id ASC
        LIMIT $5
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(after.map(|(at, _)| at))
    .bind(after.map(|(_, id)| id))
    .bind(EXPORT_COMMENT_PAGE)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, document_id, task_id, body, created_at)| ExportComment {
                id,
                workspace_id,
                document_id,
                task_id,
                body,
                created_at,
            },
        )
        .collect())
}

pub async fn export_attachments(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<ExportAttachment>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, Option<i64>, String, String)>(
        r#"
        SELECT id, workspace_id, name, mime, size_bytes, scan_status, storage_key
        FROM fvoci.app_attachments_stored_by_uploader($1)
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, workspace_id, name, mime, size_bytes, scan_status, storage_key)| {
                ExportAttachment {
                    id,
                    workspace_id,
                    name,
                    mime,
                    size_bytes,
                    scan_status,
                    storage_key,
                }
            },
        )
        .collect())
}
