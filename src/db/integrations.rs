//! Webhook rows and the per-target delivery ledger (source `repos/ops.ts`
//! `webhooks` / `webhookDeliveries`).

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, recheck_session, session_is_live, set_system, set_tenant,
};
use crate::db::documents::{membership_role, membership_role_for_update, workspace_is_live};
use crate::db::identity::{append_audit, AuditAppend};
use crate::db::workspace::WorkspaceRole;
use crate::projects::{workspace_base_permission, ProjectPermission};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationDbError {
    NotFound,
    Conflict,
    InvalidInput,
}

#[derive(Debug, Clone)]
pub struct WebhookRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub url: String,
    pub events: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewWebhook<'a> {
    pub id: Uuid,
    pub url: &'a str,
    pub events: &'a [String],
    /// Already sealed with the row context `webhook:<workspace>:<id>`.
    pub sealed_secret: &'a str,
}

pub(crate) fn has_workspace_manage(role: WorkspaceRole) -> bool {
    workspace_base_permission(role) == ProjectPermission::Manage
}

/// Read path: live credential, live workspace, current admin/owner. Any
/// failure is `NotFound` (source hides existence behind 404).
pub(crate) async fn require_manager_read(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if !session_is_live(tx, actor_user_id, session_id).await? {
        return Ok(false);
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(false);
    }
    Ok(membership_role(tx, workspace_id, actor_user_id)
        .await?
        .is_some_and(has_workspace_manage))
}

/// Write path: same checks with the actor's membership locked against a
/// concurrent demotion/removal until commit.
pub(crate) async fn require_manager_write(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(false);
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(false);
    }
    Ok(membership_role_for_update(tx, workspace_id, actor_user_id)
        .await?
        .is_some_and(has_workspace_manage))
}

type WebhookTuple = (
    Uuid,
    Uuid,
    String,
    Vec<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn webhook_row(t: WebhookTuple) -> WebhookRow {
    WebhookRow {
        id: t.0,
        workspace_id: t.1,
        url: t.2,
        events: t.3,
        created_at: t.4,
        updated_at: t.5,
    }
}

pub async fn list_webhooks(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<WebhookRow>, IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_read(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let rows: Vec<WebhookTuple> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, url, events, created_at, updated_at
        FROM fvoci.webhooks
        WHERE workspace_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows.into_iter().map(webhook_row).collect()))
}

pub async fn create_webhook(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: NewWebhook<'_>,
    client_ip: Option<&str>,
) -> Result<Result<WebhookRow, IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_write(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let row: WebhookTuple = sqlx::query_as(
        r#"
        INSERT INTO fvoci.webhooks (id, workspace_id, url, secret, events, created_by)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, workspace_id, url, events, created_at, updated_at
        "#,
    )
    .bind(input.id)
    .bind(workspace_id)
    .bind(input.url)
    .bind(input.sealed_secret)
    .bind(input.events)
    .bind(actor_user_id)
    .fetch_one(&mut *tx)
    .await?;
    // Audit is an intentional addition (source records none). No URL (it may
    // carry a receiver token) and never the secret.
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "webhook.created".into(),
            target_type: Some("webhook".into()),
            target_id: Some(input.id),
            payload: json!({ "webhookId": input.id.to_string(), "events": input.events }),
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(webhook_row(row)))
}

pub async fn remove_webhook(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    webhook_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !require_manager_write(&mut tx, workspace_id, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    // Deliveries cascade with the webhook (source purgeWebhook).
    let deleted = sqlx::query("DELETE FROM fvoci.webhooks WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(webhook_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "webhook.deleted".into(),
            target_type: Some("webhook".into()),
            target_id: Some(webhook_id),
            payload: json!({ "webhookId": webhook_id.to_string() }),
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// Hooks in `workspace_id` subscribed to `verb`, with their creator.
pub(crate) async fn subscribed_webhooks(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    verb: &str,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT id, created_by
        FROM fvoci.webhooks
        WHERE workspace_id = $1 AND $2 = ANY (events)
        ORDER BY id
        "#,
    )
    .bind(workspace_id)
    .bind(verb)
    .fetch_all(&mut **tx)
    .await
}

pub(crate) async fn enqueue_delivery(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    webhook_id: Uuid,
    event_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.webhook_deliveries
            (id, workspace_id, webhook_id, event_id, attempt, status, next_attempt_at)
        VALUES ($1, $2, $3, $4, 0, 'pending', now())
        ON CONFLICT (webhook_id, event_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(webhook_id)
    .bind(event_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueDelivery {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub webhook_id: Uuid,
    pub event_id: Uuid,
    /// Attempts already recorded before this claim.
    pub attempt: i32,
}

/// Source `claimDue`: push `next_attempt_at` to the lease end so a crashed
/// sender's rows come back after `lease`, and skip rows another claimant holds.
pub async fn claim_due_deliveries(
    pool: &PgPool,
    limit: i64,
    lease: Duration,
) -> Result<Vec<DueDelivery>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid, Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
        r#"
        UPDATE fvoci.webhook_deliveries AS d
        SET next_attempt_at = now() + make_interval(secs => $2::double precision),
            updated_at = now()
        WHERE d.id IN (
            SELECT id FROM fvoci.webhook_deliveries
            WHERE status = 'pending' AND next_attempt_at <= now()
            ORDER BY next_attempt_at, id
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING d.id, d.workspace_id, d.webhook_id, d.event_id, d.attempt
        "#,
    )
    .bind(limit)
    .bind(lease.as_secs_f64())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, workspace_id, webhook_id, event_id, attempt)| DueDelivery {
                id,
                workspace_id,
                webhook_id,
                event_id,
                attempt,
            },
        )
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    Retry { after: Duration },
    Failed,
}

/// Records one attempt for a claimed row. Fenced on the claimed attempt count
/// so a late writer from a lapsed claim cannot overwrite a newer result.
pub async fn record_delivery(
    pool: &PgPool,
    due: &DueDelivery,
    outcome: DeliveryOutcome,
    http_status: Option<u16>,
) -> Result<bool, sqlx::Error> {
    let (status, retry_secs) = match outcome {
        DeliveryOutcome::Delivered => ("delivered", None),
        DeliveryOutcome::Retry { after } => ("pending", Some(after.as_secs_f64())),
        DeliveryOutcome::Failed => ("failed", None),
    };
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, due.workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.webhook_deliveries
        SET attempt = $3,
            status = $4,
            http_status = $5,
            next_attempt_at = CASE WHEN $6::double precision IS NULL THEN NULL
                                   ELSE now() + make_interval(secs => $6::double precision) END,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status = 'pending' AND attempt = $7
        "#,
    )
    .bind(due.workspace_id)
    .bind(due.id)
    .bind(due.attempt + 1)
    .bind(status)
    .bind(http_status.map(i32::from))
    .bind(retry_secs)
    .bind(due.attempt)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(updated == 1)
}

/// What the sender needs for one claimed row, read in the row's tenant.
pub struct DeliveryTarget {
    pub url: String,
    pub sealed_secret: String,
    pub created_by: Uuid,
}

pub async fn load_delivery_target(
    tx: &mut Transaction<'_, Postgres>,
    due: &DueDelivery,
) -> Result<Option<DeliveryTarget>, sqlx::Error> {
    let row: Option<(String, String, Uuid)> = sqlx::query_as(
        r#"
        SELECT h.url, h.secret, h.created_by
        FROM fvoci.webhooks AS h
        INNER JOIN fvoci.workspaces AS w ON w.id = h.workspace_id AND w.deleted_at IS NULL
        WHERE h.workspace_id = $1 AND h.id = $2
        "#,
    )
    .bind(due.workspace_id)
    .bind(due.webhook_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(url, sealed_secret, created_by)| DeliveryTarget {
        url,
        sealed_secret,
        created_by,
    }))
}

/// Webhook creator still able to manage the workspace: present, not
/// suspended, admin or owner. Deliveries stop when the creator loses that.
pub(crate) async fn creator_can_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let active: Option<(bool,)> = sqlx::query_as(
        "SELECT deleted_at IS NULL AND suspended_at IS NULL FROM fvoci.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if !active.is_some_and(|(live,)| live) {
        return Ok(false);
    }
    Ok(membership_role(tx, workspace_id, user_id)
        .await?
        .is_some_and(has_workspace_manage))
}

pub const WEBHOOK_DELIVERY_RETENTION_DAYS: i32 = 90;
pub const GITHUB_DELIVERY_RETENTION_DAYS: i32 = 30;
pub const INTEGRATION_GC_BATCH: i64 = 5_000;

/// Source `purgeSettledBefore`: settled delivery rows older than the retention.
pub async fn purge_settled_deliveries(pool: &PgPool, days: i32) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        WITH doomed AS (
            SELECT id FROM fvoci.webhook_deliveries
            WHERE status IN ('delivered', 'failed')
              AND created_at < now() - make_interval(days => $1)
            ORDER BY created_at
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.webhook_deliveries AS d USING doomed WHERE d.id = doomed.id
        "#,
    )
    .bind(days)
    .bind(INTEGRATION_GC_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}

/// Install round trips that were never completed. Plain `DELETE … IN`: the
/// app role has no UPDATE here, which `FOR UPDATE SKIP LOCKED` would need.
pub async fn purge_expired_install_states(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.github_install_states
        WHERE nonce_hash IN (
            SELECT nonce_hash FROM fvoci.github_install_states
            WHERE expires_at <= now()
            ORDER BY expires_at
            LIMIT $1
        )
        "#,
    )
    .bind(INTEGRATION_GC_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}

/// Plain `DELETE … IN` for the same reason as the install states.
pub async fn purge_github_deliveries(pool: &PgPool, days: i32) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.github_deliveries
        WHERE delivery_id IN (
            SELECT delivery_id FROM fvoci.github_deliveries
            WHERE processed_at < now() - make_interval(days => $1)
            ORDER BY processed_at
            LIMIT $2
        )
        "#,
    )
    .bind(days)
    .bind(INTEGRATION_GC_BATCH)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}
