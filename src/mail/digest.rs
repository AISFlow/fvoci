use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::mail::templates::{digest_text, DIGEST_SUBJECT};
use crate::mail::Mailer;

const DIGEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const DIGEST_BATCH: i64 = 100;

#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Mail(crate::mail::MailSendError),
}

/// Source `sendDueDigests` with a row claim.
///
/// Recipients are claimed with `FOR UPDATE SKIP LOCKED` and `last_digest_at`
/// advances as the claim, so two processes cannot send the same digest and a
/// failed recipient backs off until the next daily sweep instead of retrying
/// every tick.
pub async fn send_due_digests(
    pool: &PgPool,
    mailer: &Mailer,
    now: DateTime<Utc>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<u32, DigestError> {
    let before =
        now - chrono::Duration::from_std(DIGEST_INTERVAL).unwrap_or(chrono::Duration::days(1));
    let due = claim_digest_due(pool, before, now).await?;
    let mut sent = 0u32;
    let mut pending = due.into_iter();
    while let Some((workspace_id, user_id, prev_last)) = pending.next() {
        if cancel.is_cancelled() {
            // Hand the unsent claims back so the next sweep sends them.
            restore_claim(pool, workspace_id, user_id, prev_last, now).await?;
            for (ws, user, prev) in pending.by_ref() {
                restore_claim(pool, ws, user, prev, now).await?;
            }
            break;
        }
        match send_claimed(pool, mailer, workspace_id, user_id, prev_last).await {
            Ok(true) => sent += 1,
            Ok(false) => {}
            Err(err) => {
                // Source keeps lastDigestAt on failure so the window is retried.
                restore_claim(pool, workspace_id, user_id, prev_last, now).await?;
                tracing::warn!(
                    message = %format!("digest: recipient deferred to the next sweep ({err})"),
                    "mail.send_failed"
                );
            }
        }
    }
    Ok(sent)
}

/// Undo a claim that did not send: put back the previous `last_digest_at`
/// unless another claim has moved it since (guarded by the claim's `now`).
async fn restore_claim(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    prev_last: Option<DateTime<Utc>>,
    claimed_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    sqlx::query(
        r#"
        UPDATE fvoci.notification_prefs
        SET last_digest_at = $3, updated_at = now()
        WHERE workspace_id = $1 AND user_id = $2 AND last_digest_at = $4
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(prev_last)
    .bind(claimed_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn claim_digest_due(
    pool: &PgPool,
    before: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<(Uuid, Uuid, Option<DateTime<Utc>>)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid, Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        WITH due AS (
            SELECT workspace_id, user_id, last_digest_at AS prev
            FROM fvoci.notification_prefs
            WHERE mail_digest = true
              AND (last_digest_at IS NULL OR last_digest_at <= $1)
            ORDER BY workspace_id, user_id
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        )
        UPDATE fvoci.notification_prefs AS p
        SET last_digest_at = $2, updated_at = now()
        FROM due
        WHERE p.workspace_id = due.workspace_id
          AND p.user_id = due.user_id
        RETURNING due.workspace_id, due.user_id, due.prev
        "#,
    )
    .bind(before)
    .bind(now)
    .bind(DIGEST_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows)
}

async fn send_claimed(
    pool: &PgPool,
    mailer: &Mailer,
    workspace_id: Uuid,
    user_id: Uuid,
    prev_last: Option<DateTime<Utc>>,
) -> Result<bool, DigestError> {
    let packed = {
        let mut tx = pool.begin().await?;
        set_system(&mut tx).await?;
        set_tenant(&mut tx, workspace_id).await?;
        let user: Option<(String,)> =
            sqlx::query_as("SELECT email FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((email,)) = user else {
            tx.commit().await?;
            return Ok(false);
        };
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)::bigint
            FROM fvoci.notifications
            WHERE workspace_id = $1
              AND user_id = $2
              AND read_at IS NULL
              AND archived_at IS NULL
              AND ($3::timestamptz IS NULL OR created_at >= $3)
            "#,
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(prev_last)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        (email, count)
    };
    if packed.1 <= 0 {
        return Ok(false);
    }
    mailer
        .send(&packed.0, DIGEST_SUBJECT, &digest_text(packed.1))
        .await
        .map_err(DigestError::Mail)?;
    Ok(true)
}
