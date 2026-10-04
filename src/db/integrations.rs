//! Webhook rows and the per-target delivery ledger (source `repos/ops.ts`
//! `webhooks` / `webhookDeliveries`).

use std::time::Duration;

use crate::db::backend::{Backend, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, recheck_session, session_is_live, set_system, set_tenant,
};
use crate::db::identity::{append_audit, AuditAppend};
use crate::db::workspace::{
    membership_role, membership_role_for_update, workspace_is_live, WorkspaceRole,
};
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

/// Internal receipt for the actual sender, separate from the preserved PG DTO.
/// The authoritative lease end distinguishes two claims of the same attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedWebhookDelivery {
    pub due: DueDelivery,
    pub claimed_until: DateTime<Utc>,
}

fn webhook_commit_error(error: crate::db::backend::CommitUnknown) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(error))
}
fn webhook_duration_us(duration: Duration) -> Result<i64, sqlx::Error> {
    let us = i64::try_from(duration.as_micros())
        .map_err(|_| sqlx::Error::Protocol("webhook duration exceeds i64 microseconds".into()))?;
    if us <= 0 {
        return Err(sqlx::Error::Protocol(
            "webhook duration must be positive at microsecond precision".into(),
        ));
    }
    Ok(us)
}
fn family_webhook_row(row: &FamilyRow) -> Result<WebhookRow, sqlx::Error> {
    Ok(WebhookRow {
        id: row.cell(0)?.id()?,
        workspace_id: row.cell(1)?.id()?,
        url: row.cell(2)?.string()?,
        events: serde_json::from_value(row.cell(3)?.value()?)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        created_at: row.cell(4)?.datetime()?,
        updated_at: row.cell(5)?.datetime()?,
    })
}

impl OperationTx<'_, '_> {
    pub(crate) async fn webhook_manager(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        write: bool,
    ) -> Result<bool, sqlx::Error> {
        if write {
            self.lock_membership_users(&[actor]).await?;
        }
        let live = if write {
            self.recheck_session(actor, credential).await?
        } else {
            self.session_is_live(actor, credential).await?
        };
        if !live || !self.workspace_is_live(workspace).await? {
            return Ok(false);
        }
        Ok(self
            .membership_role(workspace, actor, write)
            .await?
            .is_some_and(has_workspace_manage))
    }
    pub(crate) async fn webhook_creator_can_manage(
        &mut self,
        workspace: Uuid,
        user: Uuid,
    ) -> Result<bool, sqlx::Error> {
        if !self.workspace_is_live(workspace).await? {
            return Ok(false);
        }
        let active = match self {
            Self::Postgres(tx) => return creator_can_manage(tx, workspace, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.require_system_context()?;
                let rows = tx
                    .query(
                        "SELECT deleted_at IS NULL AND suspended_at IS NULL FROM users WHERE id=?1",
                        &[Cell::uuid(user)],
                    )
                    .await?;
                rows.first()
                    .map(|r| r.cell(0)?.boolean())
                    .transpose()?
                    .unwrap_or(false)
            }
        };
        if !active {
            return Ok(false);
        }
        Ok(self
            .membership_role(workspace, user, false)
            .await?
            .is_some_and(has_workspace_manage))
    }
    pub(crate) async fn webhook_list(
        &mut self,
        workspace: Uuid,
    ) -> Result<Vec<WebhookRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>Ok(sqlx::query_as::<_,WebhookTuple>("SELECT id,workspace_id,url,events,created_at,updated_at FROM fvoci.webhooks WHERE workspace_id=$1 ORDER BY created_at,id").bind(workspace).fetch_all(&mut ***tx).await?.into_iter().map(webhook_row).collect()),
            Self::SqliteFamily(tx)=> {tx.require_tenant(workspace)?;tx.query("SELECT id,workspace_id,url,events,created_at,updated_at FROM webhooks WHERE workspace_id=?1 ORDER BY created_at,id",&[Cell::uuid(workspace)]).await?.iter().map(family_webhook_row).collect()}
        }
    }
    pub(crate) async fn webhook_create(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        input: NewWebhook<'_>,
    ) -> Result<WebhookRow, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>Ok(webhook_row(sqlx::query_as::<_,WebhookTuple>("INSERT INTO fvoci.webhooks(id,workspace_id,url,secret,events,created_by) VALUES($1,$2,$3,$4,$5,$6) RETURNING id,workspace_id,url,events,created_at,updated_at").bind(input.id).bind(workspace).bind(input.url).bind(input.sealed_secret).bind(input.events).bind(actor).fetch_one(&mut ***tx).await?)),
            Self::SqliteFamily(tx)=> {tx.require_writer()?;tx.require_tenant(workspace)?;let rows=tx.query("INSERT INTO webhooks(id,workspace_id,url,secret,events,created_by) VALUES(?1,?2,?3,?4,?5,?6) RETURNING id,workspace_id,url,events,created_at,updated_at",&[Cell::uuid(input.id),Cell::uuid(workspace),Cell::text(input.url),Cell::text(input.sealed_secret),Cell::json(&json!(input.events))?,Cell::uuid(actor)]).await?;family_webhook_row(rows.first().ok_or(sqlx::Error::RowNotFound)?)}
        }
    }
    pub(crate) async fn webhook_remove(
        &mut self,
        workspace: Uuid,
        webhook: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query(
                "DELETE FROM fvoci.webhooks WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace)
            .bind(webhook)
            .execute(&mut ***tx)
            .await?
            .rows_affected()
                == 1),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                Ok(tx
                    .execute(
                        "DELETE FROM webhooks WHERE workspace_id=?1 AND id=?2",
                        &[Cell::uuid(workspace), Cell::uuid(webhook)],
                    )
                    .await?
                    == 1)
            }
        }
    }
    pub(crate) async fn webhook_subscriptions(
        &mut self,
        workspace: Uuid,
        verb: &str,
    ) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => subscribed_webhooks(tx, workspace, verb).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.require_system_context()?;
                tx.query("SELECT h.id,h.created_by FROM webhooks h WHERE h.workspace_id=?1 AND EXISTS(SELECT 1 FROM json_each(h.events) e WHERE e.type='text' AND e.value=?2) ORDER BY h.id",&[Cell::uuid(workspace),Cell::text(verb)]).await?.iter().map(|r|Ok((r.cell(0)?.id()?,r.cell(1)?.id()?))).collect()
            }
        }
    }
    pub(crate) async fn webhook_enqueue(
        &mut self,
        workspace: Uuid,
        webhook: Uuid,
        event: Uuid,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => enqueue_delivery(tx, workspace, webhook, event).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace)?;
                tx.execute("INSERT INTO webhook_deliveries(id,workspace_id,webhook_id,event_id,attempt,status,next_attempt_at) VALUES(?1,?2,?3,?4,0,'pending',(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)) ON CONFLICT(webhook_id,event_id) DO NOTHING",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(webhook),Cell::uuid(event)]).await?;
                Ok(())
            }
        }
    }
    // Source scope reads deliberately retain historical parent rows. Permission
    // stays in the shared project/wiki operations, never a second ACL policy.
    pub(crate) async fn webhook_document_project(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<Option<Uuid>>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar(
                    "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
                )
                .bind(workspace)
                .bind(document)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.require_system_context()?;
                tx.query(
                    "SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(document)],
                )
                .await?
                .first()
                .map(|r| r.cell(0)?.optional(Cell::id))
                .transpose()
            }
        }
    }
    pub(crate) async fn webhook_task_project(
        &mut self,
        workspace: Uuid,
        task: Uuid,
    ) -> Result<Option<Uuid>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar(
                    "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
                )
                .bind(workspace)
                .bind(task)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.require_system_context()?;
                tx.query(
                    "SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(task)],
                )
                .await?
                .first()
                .map(|r| r.cell(0)?.id())
                .transpose()
            }
        }
    }
    pub(crate) async fn webhook_attachment_parent(
        &mut self,
        workspace: Uuid,
        attachment: Uuid,
    ) -> Result<Option<(Option<Uuid>, Option<Uuid>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT document_id, task_id FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2").bind(workspace).bind(attachment).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{tx.require_tenant(workspace)?;tx.require_system_context()?;tx.query("SELECT document_id,task_id FROM attachments WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(attachment)]).await?.first().map(|r|Ok((r.cell(0)?.optional(Cell::id)?,r.cell(1)?.optional(Cell::id)?))).transpose()}
        }
    }
    pub(crate) async fn webhook_comment_parent(
        &mut self,
        workspace: Uuid,
        comment: Uuid,
    ) -> Result<Option<(Option<Uuid>, Option<Uuid>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT document_id, task_id FROM fvoci.comments WHERE workspace_id = $1 AND id = $2").bind(workspace).bind(comment).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{tx.require_tenant(workspace)?;tx.require_system_context()?;tx.query("SELECT document_id,task_id FROM comments WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(comment)]).await?.first().map(|r|Ok((r.cell(0)?.optional(Cell::id)?,r.cell(1)?.optional(Cell::id)?))).transpose()}
        }
    }
    pub(crate) async fn webhook_claim_due(
        &mut self,
        limit: i64,
        lease: Duration,
    ) -> Result<Vec<ClaimedWebhookDelivery>, sqlx::Error> {
        let lease_us = webhook_duration_us(lease)?;
        if limit <= 0 {
            return Err(sqlx::Error::Protocol(
                "webhook claim limit must be positive".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                let rows:Vec<(Uuid,Uuid,Uuid,Uuid,i32,DateTime<Utc>)>=sqlx::query_as(r#"UPDATE fvoci.webhook_deliveries AS d
                    SET next_attempt_at=now()+make_interval(secs=>$2::double precision),updated_at=now()
                    WHERE d.id IN (SELECT id FROM fvoci.webhook_deliveries WHERE status='pending' AND next_attempt_at<=now() ORDER BY next_attempt_at,id LIMIT $1 FOR UPDATE SKIP LOCKED)
                    RETURNING d.id,d.workspace_id,d.webhook_id,d.event_id,d.attempt,d.next_attempt_at"#).bind(limit).bind(lease_us as f64/1_000_000.0).fetch_all(&mut ***tx).await?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(id, workspace_id, webhook_id, event_id, attempt, claimed_until)| {
                            ClaimedWebhookDelivery {
                                due: DueDelivery {
                                    id,
                                    workspace_id,
                                    webhook_id,
                                    event_id,
                                    attempt,
                                },
                                claimed_until,
                            }
                        },
                    )
                    .collect())
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_writer()?;
                let now=tx.query("SELECT (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)",&[]).await?.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.integer()?;
                let until = now
                    .checked_add(lease_us)
                    .ok_or_else(|| sqlx::Error::Protocol("webhook lease time overflow".into()))?;
                let rows=tx.query("SELECT id,workspace_id,webhook_id,event_id,attempt FROM webhook_deliveries WHERE status='pending' AND next_attempt_at<=?1 ORDER BY next_attempt_at,id LIMIT ?2",&[Cell::Integer(now),Cell::Integer(limit)]).await?;
                let mut result = Vec::new();
                for row in rows {
                    let due = DueDelivery {
                        id: row.cell(0)?.id()?,
                        workspace_id: row.cell(1)?.id()?,
                        webhook_id: row.cell(2)?.id()?,
                        event_id: row.cell(3)?.id()?,
                        attempt: row.cell(4)?.int32()?,
                    };
                    let changed=tx.execute("UPDATE webhook_deliveries SET next_attempt_at=?3,updated_at=?4 WHERE workspace_id=?1 AND id=?2 AND status='pending' AND next_attempt_at<=?4",&[Cell::uuid(due.workspace_id),Cell::uuid(due.id),Cell::Integer(until),Cell::Integer(now)]).await?;
                    if changed != 1 {
                        return Err(sqlx::Error::Protocol(
                            "webhook claim changed under writer reservation".into(),
                        ));
                    }
                    result.push(ClaimedWebhookDelivery {
                        due,
                        claimed_until: DateTime::from_timestamp_micros(until).ok_or_else(|| {
                            sqlx::Error::Protocol("webhook lease instant out of range".into())
                        })?,
                    });
                }
                Ok(result)
            }
        }
    }
    pub(crate) async fn webhook_claim_is_current(
        &mut self,
        claim: &ClaimedWebhookDelivery,
    ) -> Result<bool, sqlx::Error> {
        let d = &claim.due;
        match self {
            Self::Postgres(tx)=>Ok(sqlx::query_scalar::<_,bool>("SELECT true FROM fvoci.webhook_deliveries WHERE workspace_id=$1 AND id=$2 AND webhook_id=$3 AND event_id=$4 AND status='pending' AND attempt=$5 AND next_attempt_at=$6").bind(d.workspace_id).bind(d.id).bind(d.webhook_id).bind(d.event_id).bind(d.attempt).bind(claim.claimed_until).fetch_optional(&mut ***tx).await?.unwrap_or(false)),
            Self::SqliteFamily(tx)=>{tx.require_tenant(d.workspace_id)?;tx.require_system_context()?;tx.query("SELECT 1 FROM webhook_deliveries WHERE workspace_id=?1 AND id=?2 AND webhook_id=?3 AND event_id=?4 AND status='pending' AND attempt=?5 AND next_attempt_at=?6",&[Cell::uuid(d.workspace_id),Cell::uuid(d.id),Cell::uuid(d.webhook_id),Cell::uuid(d.event_id),Cell::Integer(i64::from(d.attempt)),Cell::instant(claim.claimed_until)?]).await.map(|r|!r.is_empty())}
        }
    }
    pub(crate) async fn webhook_target(
        &mut self,
        due: &DueDelivery,
    ) -> Result<Option<DeliveryTarget>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => load_delivery_target(tx, due).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(due.workspace_id)?;
                tx.require_system_context()?;
                tx.query("SELECT h.url,h.secret,h.created_by FROM webhooks h INNER JOIN workspaces w ON w.id=h.workspace_id AND w.deleted_at IS NULL WHERE h.workspace_id=?1 AND h.id=?2",&[Cell::uuid(due.workspace_id),Cell::uuid(due.webhook_id)]).await?.first().map(|r|Ok(DeliveryTarget{url:r.cell(0)?.string()?,sealed_secret:r.cell(1)?.string()?,created_by:r.cell(2)?.id()?})).transpose()
            }
        }
    }
    pub(crate) async fn webhook_record(
        &mut self,
        claim: &ClaimedWebhookDelivery,
        outcome: DeliveryOutcome,
        http_status: Option<u16>,
    ) -> Result<bool, sqlx::Error> {
        let d = &claim.due;
        let attempt = d
            .attempt
            .checked_add(1)
            .ok_or_else(|| sqlx::Error::Protocol("webhook attempt overflow".into()))?;
        let (status, after) = match outcome {
            DeliveryOutcome::Delivered => ("delivered", None),
            DeliveryOutcome::Failed => ("failed", None),
            DeliveryOutcome::Retry { after } => ("pending", Some(webhook_duration_us(after)?)),
        };
        // Receipt equality, not expiry alone: an expired but unreplaced claim
        // may still finish; an old acknowledgement cannot mutate a replacement.
        match self {
            Self::Postgres(tx)=>Ok(sqlx::query("UPDATE fvoci.webhook_deliveries SET attempt=$3,status=$4,http_status=$5,next_attempt_at=CASE WHEN $6::double precision IS NULL THEN NULL ELSE now()+make_interval(secs=>$6::double precision) END,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND status='pending' AND attempt=$7 AND next_attempt_at=$8 AND webhook_id=$9 AND event_id=$10").bind(d.workspace_id).bind(d.id).bind(attempt).bind(status).bind(http_status.map(i32::from)).bind(after.map(|us|us as f64/1_000_000.0)).bind(d.attempt).bind(claim.claimed_until).bind(d.webhook_id).bind(d.event_id).execute(&mut ***tx).await?.rows_affected()==1),
            Self::SqliteFamily(tx)=>{tx.require_writer()?;tx.require_tenant(d.workspace_id)?;tx.require_system_context()?;let changed=tx.execute("UPDATE webhook_deliveries SET attempt=?3,status=?4,http_status=?5,next_attempt_at=CASE WHEN ?6 IS NULL THEN NULL ELSE (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)+?6 END,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2 AND status='pending' AND attempt=?7 AND next_attempt_at=?8 AND webhook_id=?9 AND event_id=?10",&[Cell::uuid(d.workspace_id),Cell::uuid(d.id),Cell::Integer(i64::from(attempt)),Cell::text(status),http_status.map(|s|Cell::Integer(i64::from(s))).unwrap_or(Cell::Null),after.map(Cell::Integer).unwrap_or(Cell::Null),Cell::Integer(i64::from(d.attempt)),Cell::instant(claim.claimed_until)?,Cell::uuid(d.webhook_id),Cell::uuid(d.event_id)]).await?;Ok(changed==1)}
        }
    }
    pub(crate) async fn webhook_purge_settled(&mut self, days: i32) -> Result<u64, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>Ok(sqlx::query(r#"WITH doomed AS (SELECT id FROM fvoci.webhook_deliveries WHERE status IN ('delivered','failed') AND created_at<now()-make_interval(days=>$1) ORDER BY created_at LIMIT $2 FOR UPDATE SKIP LOCKED) DELETE FROM fvoci.webhook_deliveries AS d USING doomed WHERE d.id=doomed.id"#).bind(days).bind(INTEGRATION_GC_BATCH).execute(&mut ***tx).await?.rows_affected()),
            Self::SqliteFamily(tx)=>{tx.require_system_context()?;tx.require_writer()?;tx.execute("DELETE FROM webhook_deliveries WHERE id IN (SELECT id FROM webhook_deliveries WHERE status IN ('delivered','failed') AND created_at<(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?1 ORDER BY created_at LIMIT ?2)",&[Cell::Integer(i64::from(days)*86_400_000_000),Cell::Integer(INTEGRATION_GC_BATCH)]).await}
        }
    }
}

pub async fn list_webhooks_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<Vec<WebhookRow>, IntegrationDbError>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx
        .operation()
        .webhook_manager(workspace, actor, credential, false)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let rows = tx.operation().webhook_list(workspace).await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(Ok(rows))
}
pub async fn create_webhook_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    input: NewWebhook<'_>,
    client_ip: Option<&str>,
) -> Result<Result<WebhookRow, IntegrationDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx
        .operation()
        .webhook_manager(workspace, actor, credential, true)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    let id = input.id;
    let events = input.events.to_vec();
    let row = tx
        .operation()
        .webhook_create(workspace, actor, input)
        .await?;
    tx.operation()
        .append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: "webhook.created".into(),
            target_type: Some("webhook".into()),
            target_id: Some(id),
            payload: json!({"webhookId":id.to_string(),"events":events}),
            ip: client_ip.map(str::to_string),
        })
        .await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(Ok(row))
}
pub async fn remove_webhook_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    webhook: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), IntegrationDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx
        .operation()
        .webhook_manager(workspace, actor, credential, true)
        .await?
        || !tx.operation().webhook_remove(workspace, webhook).await?
    {
        tx.rollback().await?;
        return Ok(Err(IntegrationDbError::NotFound));
    }
    tx.operation()
        .append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: "webhook.deleted".into(),
            target_type: Some("webhook".into()),
            target_id: Some(webhook),
            payload: json!({"webhookId":webhook.to_string()}),
            ip: client_ip.map(str::to_string),
        })
        .await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(Ok(()))
}
pub(crate) async fn claim_due_webhooks_backend(
    backend: &Backend,
    limit: i64,
    lease: Duration,
) -> Result<Vec<ClaimedWebhookDelivery>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    let rows = tx.operation().webhook_claim_due(limit, lease).await?;
    tx.operation().restore_system(previous).await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(rows)
}
pub(crate) async fn record_webhook_backend(
    backend: &Backend,
    claim: &ClaimedWebhookDelivery,
    outcome: DeliveryOutcome,
    status: Option<u16>,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(claim.due.workspace_id).await?;
    let previous = tx.operation().set_system().await?;
    let recorded = tx
        .operation()
        .webhook_record(claim, outcome, status)
        .await?;
    tx.operation().restore_system(previous).await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(recorded)
}
pub async fn purge_settled_deliveries_backend(
    backend: &Backend,
    days: i32,
) -> Result<u64, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    let deleted = tx.operation().webhook_purge_settled(days).await?;
    tx.operation().restore_system(previous).await?;
    tx.commit().await.map_err(webhook_commit_error)?;
    Ok(deleted)
}

#[cfg(test)]
pub(crate) mod webhook_family_fixture {
    use super::*;
    pub(crate) use crate::db::notifications::family_runtime_fixture::Fixture;
    pub(crate) const SECRET: &str = "synthetic-webhook-key";
    pub(crate) fn keys() -> std::sync::Arc<crate::auth::password::Keyring> {
        std::sync::Arc::new(
            crate::auth::password::Keyring::parse(
                r#"{"k1":"0101010101010101010101010101010101010101010101010101010101010101"}"#,
                "k1",
            )
            .unwrap(),
        )
    }
    pub(crate) async fn hook(f: &Fixture, url: &str) -> Uuid {
        sqlx::query("UPDATE memberships SET role='admin' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let id = Uuid::now_v7();
        let sealed = crate::secret_box::seal(
            &keys(),
            SECRET,
            &crate::integrations::webhooks::webhook_secret_context(f.workspace, id),
        )
        .unwrap();
        let events = vec![
            "comment.created".into(),
            "document.updated".into(),
            "task.deleted".into(),
        ];
        assert!(create_webhook_backend(
            &f.backend,
            f.workspace,
            f.user,
            f.credential,
            NewWebhook {
                id,
                url,
                events: &events,
                sealed_secret: &sealed
            },
            None
        )
        .await
        .unwrap()
        .is_ok());
        id
    }
    pub(crate) async fn enqueue(f: &Fixture, hook: Uuid, event: Uuid) {
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let p = tx.operation().set_system().await.unwrap();
        tx.operation()
            .webhook_enqueue(f.workspace, hook, event)
            .await
            .unwrap();
        tx.operation().restore_system(p).await.unwrap();
        tx.commit().await.unwrap();
    }
    pub(crate) async fn duplicate_counts(tx: &mut OperationTx<'_, '_>, event: Uuid) -> (i64, i64) {
        let OperationTx::SqliteFamily(family) = tx else {
            unreachable!()
        };
        let rows = family.query("SELECT (SELECT count(*) FROM processed_events WHERE consumer='webhooks' AND event_id=?1),(SELECT count(*) FROM webhook_deliveries WHERE event_id=?1)", &[Cell::uuid(event)]).await.unwrap();
        (
            rows[0].cell(0).unwrap().integer().unwrap(),
            rows[0].cell(1).unwrap().integer().unwrap(),
        )
    }
    pub(crate) async fn claim(f: &Fixture) -> ClaimedWebhookDelivery {
        claim_due_webhooks_backend(&f.backend, 1, Duration::from_secs(240))
            .await
            .unwrap()
            .pop()
            .unwrap()
    }
    pub(crate) async fn row(f: &Fixture, id: Uuid) -> (i32, String, Option<i32>, Option<i64>) {
        sqlx::query_as(
            "SELECT attempt,status,http_status,next_attempt_at FROM webhook_deliveries WHERE id=?1",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap()
    }
}

#[cfg(test)]
mod webhook_backend_regressions {
    use super::webhook_family_fixture::*;
    use super::*;

    #[tokio::test]
    async fn actual_family_reclaimed_attempt_rejects_old_acknowledgement() {
        let f = Fixture::new().await;
        let hook = hook(&f, "http://127.0.0.1:1/unused").await;
        let event = f.append_comment_event("comment.created").await;
        enqueue(&f, hook, event.id).await;
        let old = claim_due_webhooks_backend(&f.backend, 1, Duration::from_micros(1))
            .await
            .unwrap()
            .pop()
            .unwrap();
        // Observe the actual DB clock crossing this real positive lease. No
        // manual expiry mutation, clock injection or second TTL is needed.
        tokio::time::timeout(Duration::from_secs(1),async {
            loop {
                let now:i64=sqlx::query_scalar("SELECT (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)").fetch_one(&f.pool).await.unwrap();
                if now>=old.claimed_until.timestamp_micros(){break}
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        let new = claim(&f).await;
        assert_eq!(old.due.attempt, new.due.attempt);
        assert!(new.claimed_until > old.claimed_until);
        let accepted =
            record_webhook_backend(&f.backend, &old, DeliveryOutcome::Delivered, Some(204))
                .await
                .unwrap();
        let state = row(&f, new.due.id).await;
        eprintln!("webhook stale control: old_until_us={} new_until_us={} old_attempt={} new_attempt={} old_ack_accepted={} current_state={:?}",old.claimed_until.timestamp_micros(),new.claimed_until.timestamp_micros(),old.due.attempt,new.due.attempt,accepted,state);
        let replacement_accepted =
            record_webhook_backend(&f.backend, &new, DeliveryOutcome::Delivered, Some(204))
                .await
                .unwrap();
        let replacement_state = row(&f, new.due.id).await;
        // Cleanup before the deliberate old-source regression assertion so
        // even the allocated failure control has no retained fixture resource.
        f.finish().await;
        assert!(
            !accepted,
            "old claim ACK changed a reclaimed delivery with the same attempt"
        );
        assert_eq!(
            state,
            (
                0,
                "pending".into(),
                None,
                Some(new.claimed_until.timestamp_micros())
            )
        );
        assert!(
            replacement_accepted,
            "current receipt must still record an actual outcome"
        );
        assert_eq!(replacement_state, (1, "delivered".into(), Some(204), None));
    }
}

#[cfg(test)]
mod webhook_operation_regressions {
    use super::webhook_family_fixture::*;
    use super::*;

    #[tokio::test]
    async fn actual_family_claims_two_pools_expiry_and_checked_microsecond_receipts() {
        let f = Fixture::new().await;
        let h = hook(&f, "http://127.0.0.1:1/unused").await;
        let event = f.append_comment_event("comment.created").await;
        enqueue(&f, h, event.id).await;
        let other_pool = crate::db::pool::connect_sqlite_app(&f.dir.join("test.sqlite"), 1)
            .await
            .unwrap();
        let other = Backend::Sqlite(other_pool.clone());
        let (a, b) = tokio::join!(
            claim_due_webhooks_backend(&f.backend, 1, Duration::from_secs(240)),
            claim_due_webhooks_backend(&other, 1, Duration::from_secs(240))
        );
        let mut claims = a.unwrap();
        claims.extend(b.unwrap());
        assert_eq!(claims.len(), 1);
        let receipt = claims.pop().unwrap();
        assert_eq!(
            row(&f, receipt.due.id).await.3,
            Some(receipt.claimed_until.timestamp_micros())
        );
        assert!(claim_due_webhooks_backend(&f.backend, 1, Duration::ZERO)
            .await
            .is_err());
        assert!(
            claim_due_webhooks_backend(&f.backend, 1, Duration::from_nanos(1))
                .await
                .is_err()
        );
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.other_workspace).await.unwrap();
        assert!(tx
            .operation()
            .webhook_claim_is_current(&receipt)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        // An expired receipt may still record if it has not been replaced.
        // Use a precise durable timestamp to prove NULL/us and that expiry
        // itself is not a second rejection/TTL rule.
        let instant = DateTime::from_timestamp_micros(1_234_567).unwrap();
        sqlx::query("UPDATE webhook_deliveries SET next_attempt_at=?2 WHERE id=?1")
            .bind(receipt.due.id.as_bytes().as_slice())
            .bind(instant.timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        let expired = ClaimedWebhookDelivery {
            due: receipt.due.clone(),
            claimed_until: instant,
        };
        assert!(record_webhook_backend(
            &f.backend,
            &expired,
            DeliveryOutcome::Delivered,
            Some(204)
        )
        .await
        .unwrap());
        assert_eq!(
            row(&f, expired.due.id).await,
            (1, "delivered".into(), Some(204), None)
        );
        other_pool.close().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_family_cancelled_fanout_writer_rolls_back_and_reuses_connection() {
        let f = Fixture::new().await;
        let h = hook(&f, "http://127.0.0.1:1/unused").await;
        let event = f.append_comment_event("comment.created").await;
        let backend = f.backend.clone();
        let workspace = f.workspace;
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let ready = entered.clone();
        let task = tokio::spawn(async move {
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation().set_tenant(workspace).await.unwrap();
            tx.operation().set_system().await.unwrap();
            tx.operation()
                .webhook_enqueue(workspace, h, event.id)
                .await
                .unwrap();
            ready.notify_one();
            std::future::pending::<()>().await;
            tx.commit().await.unwrap();
        });
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE webhook_id=?1")
                .bind(h.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(n, 0);
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.other_workspace).await.unwrap();
        tx.operation().set_system().await.unwrap();
        assert!(tx
            .operation()
            .webhook_subscriptions(f.workspace, "comment.created")
            .await
            .is_err());
        tx.rollback().await.unwrap();
        enqueue(&f, h, event.id).await;
        assert_eq!(
            claim_due_webhooks_backend(&f.backend, 1, Duration::from_secs(240))
                .await
                .unwrap()
                .len(),
            1
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_family_webhook_crud_current_credential_audit_and_retention() {
        let f = Fixture::new().await;
        let h = hook(&f, "http://127.0.0.1:1/unused").await;
        let listed = list_webhooks_backend(&f.backend, f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, h);
        assert_eq!(
            listed[0].events,
            vec!["comment.created", "document.updated", "task.deleted"]
        );
        assert_eq!(listed[0].created_at.timestamp_subsec_nanos() % 1000, 0);
        assert!(matches!(
            list_webhooks_backend(&f.backend, f.other_workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(IntegrationDbError::NotFound)
        ));
        sqlx::query("DELETE FROM sessions WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            remove_webhook_backend(&f.backend, f.workspace, f.user, f.credential, h, None)
                .await
                .unwrap(),
            Err(IntegrationDbError::NotFound)
        ));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM webhooks WHERE id=?1")
            .bind(h.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let audit: String = sqlx::query_scalar(
            "SELECT payload FROM audit_log WHERE verb='webhook.created' AND target_id=?1",
        )
        .bind(h.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert!(!audit.contains("http:"));
        assert!(!audit.contains(SECRET));
        assert!(!audit.contains("enc:v2:"));
        let e = f.append_comment_event("comment.created").await;
        enqueue(&f, h, e.id).await;
        let c = claim(&f).await;
        assert!(
            record_webhook_backend(&f.backend, &c, DeliveryOutcome::Delivered, Some(204))
                .await
                .unwrap()
        );
        sqlx::query("UPDATE webhook_deliveries SET created_at=1 WHERE id=?1")
            .bind(c.due.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let pending = f.append_comment_event("comment.created").await;
        enqueue(&f, h, pending.id).await;
        sqlx::query("UPDATE webhook_deliveries SET created_at=1 WHERE event_id=?1")
            .bind(pending.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            purge_settled_deliveries_backend(&f.backend, 90)
                .await
                .unwrap(),
            1
        );
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM webhook_deliveries WHERE webhook_id=?1 AND status='pending'",
        )
        .bind(h.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(remaining, 1);
        assert_eq!(
            purge_settled_deliveries_backend(&f.backend, 90)
                .await
                .unwrap(),
            0
        );
        f.finish().await;
    }
}

// Selected outbound status sync. Background authority is the original tenant
// association, not an invented actor/manage gate; callers own transaction scope.
impl OperationTx<'_, '_> {
    pub(crate) async fn github_sync_target(
        &mut self,
        workspace: Uuid,
        task: Uuid,
    ) -> Result<Option<(String, i32, String, String)>, sqlx::Error> {
        if !self.workspace_is_live(workspace).await? {
            return Ok(None);
        }
        match self {
            Self::Postgres(tx) => {
                sqlx::query_as(
                    r#"
                    SELECT l.repo, l.issue_number, s.category, i.installation_id
                    FROM fvoci.github_issue_links AS l
                    INNER JOIN fvoci.tasks AS t ON t.workspace_id = l.workspace_id AND t.id = l.task_id
                    INNER JOIN fvoci.statuses AS s ON s.workspace_id = t.workspace_id AND s.id = t.status_id
                    INNER JOIN fvoci.github_installations AS i ON i.workspace_id = l.workspace_id
                    WHERE l.workspace_id = $1 AND l.task_id = $2 AND t.deleted_at IS NULL
                    "#,
                )
                .bind(workspace)
                .bind(task)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows = tx.query(
                    "SELECT l.repo,l.issue_number,s.category,i.installation_id FROM github_issue_links l JOIN tasks t ON t.workspace_id=l.workspace_id AND t.id=l.task_id JOIN statuses s ON s.workspace_id=t.workspace_id AND s.id=t.status_id JOIN github_installations i ON i.workspace_id=l.workspace_id WHERE l.workspace_id=?1 AND l.task_id=?2 AND t.deleted_at IS NULL",
                    &[Cell::uuid(workspace), Cell::uuid(task)],
                ).await?;
                rows.first().map(|r| {
                    let issue = i32::try_from(r.cell(1)?.integer()?)
                        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
                    Ok((r.cell(0)?.string()?, issue, r.cell(2)?.string()?, r.cell(3)?.string()?))
                }).transpose()
            }
        }
    }
}
