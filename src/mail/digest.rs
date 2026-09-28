use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::mail::templates::{digest_text, DIGEST_SUBJECT};
use crate::mail::{smtp, Mailer};

const DIGEST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const DIGEST_BATCH: i64 = 100;
/// Wall-clock bound for one sweep. Sends run one after another in the
/// maintenance task, which also runs the upload GC and revision sweeps; the
/// rows a sweep does not reach stay due for the next daily sweep.
const DIGEST_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Failed sends in a row, across batches, after which the sweep takes SMTP
/// to be down and stops. A sent digest or a refusal of one recipient's
/// mailbox resets the count: both show the relay is up and serving.
///
/// Known limitation: a row with nothing to send does not reset the count.
/// Five recipients whose sends fail every day (a lasting 4xx such as
/// `452 4.2.2` over quota, or a 5xx policy refusal), with only such rows
/// between them, still end the walk. It ends at the same row every day, so
/// the rows after it are not served while those recipients keep failing.
const DIGEST_DOWN_STREAK: u32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Mail(crate::mail::MailSendError),
}

type DigestClaim = (Uuid, Uuid, Option<DateTime<Utc>>);

/// Source `sendDueDigests` with a row claim.
///
/// Recipients are claimed with `FOR UPDATE SKIP LOCKED` and `last_digest_at`
/// advances as the claim, so two processes cannot send the same digest and a
/// failed recipient backs off until the next daily sweep instead of retrying
/// every tick.
///
/// The sweep walks the due rows in `(workspace_id, user_id)` order, one claim
/// batch after another, so every due row is served, not only the first
/// batch. A failed claim is handed back right after its send; it lies behind
/// the walk, so this sweep does not claim it again. The walk stops after a short
/// batch, on cancel, after `DIGEST_TIME_BUDGET`, or after
/// `DIGEST_DOWN_STREAK` failed sends in a row (SMTP is most likely down); the
/// claims not tried are handed back. One recipient's 4xx counts towards the
/// streak but cannot end the walk on its own.
pub async fn send_due_digests(
    pool: &PgPool,
    mailer: &Mailer,
    now: DateTime<Utc>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<u32, DigestError> {
    let before =
        now - chrono::Duration::from_std(DIGEST_INTERVAL).unwrap_or(chrono::Duration::days(1));
    let deadline = std::time::Instant::now() + DIGEST_TIME_BUDGET;
    let stop = |down_streak: u32| {
        down_streak >= DIGEST_DOWN_STREAK
            || cancel.is_cancelled()
            || std::time::Instant::now() >= deadline
    };
    let mut after: Option<(Uuid, Uuid)> = None;
    let mut sent = 0u32;
    let mut down_streak = 0u32;
    while !stop(down_streak) {
        let due = claim_digest_due(pool, before, now, after).await?;
        // UPDATE .. RETURNING has no order: the walk resumes after the largest key.
        let Some(last) = due.iter().map(|(ws, user, _)| (*ws, *user)).max() else {
            break;
        };
        after = Some(last);
        let short = (due.len() as i64) < DIGEST_BATCH;

        let mut pending = due.into_iter();
        while let Some(claim) = pending.next() {
            if stop(down_streak) {
                // Hand the claims not tried back so the next sweep sends them.
                for (workspace_id, user_id, prev_last) in std::iter::once(claim).chain(pending) {
                    restore_claim(pool, workspace_id, user_id, prev_last, now).await?;
                }
                break;
            }
            let (workspace_id, user_id, prev_last) = claim;
            match send_claimed(pool, mailer, workspace_id, user_id, prev_last).await {
                Ok(true) => {
                    sent += 1;
                    down_streak = 0;
                }
                Ok(false) => {}
                Err(err) => {
                    // Source keeps lastDigestAt on failure so the window is
                    // retried. Hand the claim back at once: it lies behind
                    // the walk, so this sweep does not claim it again, and a
                    // sweep dropped later in the batch still leaves it due.
                    restore_claim(pool, workspace_id, user_id, prev_last, now).await?;
                    if matches!(&err, DigestError::Mail(mail) if smtp::is_final_for_recipient(&mail.code))
                    {
                        down_streak = 0;
                    } else {
                        down_streak += 1;
                    }
                    tracing::warn!(
                        message = %format!("digest: recipient deferred to the next sweep ({err})"),
                        "mail.send_failed"
                    );
                }
            }
        }
        if short {
            break;
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

/// Claim the next due batch after `after` in `(workspace_id, user_id)` order.
async fn claim_digest_due(
    pool: &PgPool,
    before: DateTime<Utc>,
    now: DateTime<Utc>,
    after: Option<(Uuid, Uuid)>,
) -> Result<Vec<DigestClaim>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<DigestClaim> = sqlx::query_as(
        r#"
        WITH due AS (
            SELECT workspace_id, user_id, last_digest_at AS prev
            FROM fvoci.notification_prefs
            WHERE mail_digest = true
              AND (last_digest_at IS NULL OR last_digest_at <= $1)
              AND ($4::uuid IS NULL OR (workspace_id, user_id) > ($4::uuid, $5::uuid))
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
    .bind(after.map(|(workspace_id, _)| workspace_id))
    .bind(after.map(|(_, user_id)| user_id))
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
