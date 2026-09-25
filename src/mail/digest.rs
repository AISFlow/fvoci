use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::mail::templates::{digest_text, DIGEST_SUBJECT};
use crate::mail::Mailer;

const DIGEST_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const DIGEST_TICK: Duration = Duration::from_secs(60);
const DIGEST_BATCH: i64 = 100;

pub struct DigestSweepHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

impl DigestSweepHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("digest sweep join failed: {err}"))?;
        Ok(())
    }
}

pub fn spawn_digest_sweep(pool: PgPool, mailer: Arc<Mailer>) -> DigestSweepHandle {
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let join = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DIGEST_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = child.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(err) = send_due_digests(&pool, &mailer, Utc::now()).await {
                        warn!(error = %err, "digest sweep failed");
                    }
                }
            }
        }
    });
    DigestSweepHandle { cancel, join }
}

#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Mail(crate::mail::MailSendError),
}

/// Source `sendDueDigests`: one failure skips that recipient (last_digest_at
/// stays put) so the rest of the batch still goes out.
pub async fn send_due_digests(
    pool: &PgPool,
    mailer: &Mailer,
    now: DateTime<Utc>,
) -> Result<u32, DigestError> {
    let before =
        now - chrono::Duration::from_std(DIGEST_INTERVAL).unwrap_or(chrono::Duration::days(1));
    let due = list_digest_due(pool, before).await?;
    let mut sent = 0u32;
    for (workspace_id, user_id) in due {
        match send_one(pool, mailer, workspace_id, user_id, now).await {
            Ok(true) => sent += 1,
            Ok(false) => {}
            Err(err) => {
                warn!(
                    message = %format!("digest: recipient skipped ({err})"),
                    "mail.send_failed"
                );
            }
        }
    }
    Ok(sent)
}

async fn list_digest_due(
    pool: &PgPool,
    before: DateTime<Utc>,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT workspace_id, user_id
        FROM fvoci.notification_prefs
        WHERE mail_digest = true
          AND (last_digest_at IS NULL OR last_digest_at <= $1)
        ORDER BY workspace_id, user_id
        LIMIT $2
        "#,
    )
    .bind(before)
    .bind(DIGEST_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows)
}

async fn send_one(
    pool: &PgPool,
    mailer: &Mailer,
    workspace_id: Uuid,
    user_id: Uuid,
    now: DateTime<Utc>,
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
            update_last_digest_at(&mut tx, workspace_id, user_id, now).await?;
            tx.commit().await?;
            return Ok(false);
        };
        let last_digest: Option<DateTime<Utc>> = sqlx::query_scalar(
            r#"
            SELECT last_digest_at
            FROM fvoci.notification_prefs
            WHERE workspace_id = $1 AND user_id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
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
        .bind(last_digest)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        (email, count)
    };
    let did_send = packed.1 > 0;
    if did_send {
        mailer
            .send(&packed.0, DIGEST_SUBJECT, &digest_text(packed.1))
            .await
            .map_err(DigestError::Mail)?;
    }
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    update_last_digest_at(&mut tx, workspace_id, user_id, now).await?;
    tx.commit().await?;
    Ok(did_send)
}

async fn update_last_digest_at(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE fvoci.notification_prefs
        SET last_digest_at = $3, updated_at = now()
        WHERE workspace_id = $1 AND user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
