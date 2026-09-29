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
///
/// Known limitation: every sweep walks from the first key, and the rows it
/// served are due again the next day, so a sweep that runs out of budget
/// ends at about the same row every day; the rows after it are not served
/// while that lasts, like the streak stops below.
const DIGEST_TIME_BUDGET: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Sends that failed with a 4xx, a timeout, a connection failure or a local
/// error, counted across batches, after which the sweep takes SMTP to be
/// down and stops. A 5xx does not count here: the relay answered (see
/// `DIGEST_REFUSAL_STREAK`). A sent digest or a refusal final for one
/// recipient (`smtp::is_final_for_recipient`) resets the count: a mailbox
/// refusal (an enhanced X.1/X.2 mailbox code, a bare 551) shows the relay is
/// up and serving like a sent digest; an address that does not parse never
/// reaches the relay and also resets it. A row with nothing to send and a
/// 5xx leave it as it is.
///
/// Known limitation: five recipients whose sends fail every day with a
/// lasting 4xx (such as `452 4.2.2` over quota), with only rows that send
/// nothing or get a 5xx between them, still end the walk. It ends at the
/// same row every day, so the rows after it are not served while those
/// recipients keep failing.
const DIGEST_DOWN_STREAK: u32 = 5;
/// Sends refused with a 5xx not known to be about the recipient
/// (`smtp::is_unclassified_refusal`), counted across batches, after which
/// the sweep takes the relay to refuse every recipient (a daily sending
/// limit, a refused sender) and stops. It is larger than
/// `DIGEST_DOWN_STREAK` because relays that send no enhanced status codes
/// (Exim by default, cPanel, qmail) refuse an unknown user with a bare 550,
/// which the classifier cannot tell from a relay-wide refusal. The same
/// events reset it as `DIGEST_DOWN_STREAK`; a 4xx, timeout or connection
/// failure leaves it as it is.
///
/// Known limitation: twenty such refusals every day (for example unknown
/// users on such a relay), with only rows that send nothing or fail without
/// a 5xx between them, end the walk at the same row every day, like
/// `DIGEST_DOWN_STREAK`.
const DIGEST_REFUSAL_STREAK: u32 = 20;

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
/// every tick. A send whose acceptance was not seen (the session timeout, a
/// connection dropped after DATA) is handed back like any failed send, so its
/// window is counted again the next day and the recipient may be told about
/// it twice. A crash, or a failed hand-back (`restore_claim`), leaves the
/// claimed rows unsent until the next day's sweep, whose count window then
/// starts at the claim.
///
/// The sweep walks the due rows in `(workspace_id, user_id)` order, one claim
/// batch after another, so every due row is served, not only the first
/// batch. A failed claim is handed back right after its send; it lies behind
/// the walk, so this sweep does not claim it again. The walk stops after a short
/// batch, on cancel, after `DIGEST_TIME_BUDGET`, after `DIGEST_DOWN_STREAK`
/// sends that failed without a reply or with a 4xx (SMTP is most likely
/// down), or after `DIGEST_REFUSAL_STREAK` unclassified 5xx refusals (the
/// relay most likely refuses everyone), in both cases without a sent digest
/// or a final refusal between them; the claims not tried are handed back.
/// One recipient's failure counts towards a streak but cannot end the walk
/// on its own.
pub async fn send_due_digests(
    pool: &PgPool,
    mailer: &Mailer,
    now: DateTime<Utc>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<u32, DigestError> {
    let before =
        now - chrono::Duration::from_std(DIGEST_INTERVAL).unwrap_or(chrono::Duration::days(1));
    let deadline = std::time::Instant::now() + DIGEST_TIME_BUDGET;
    let mut streaks = SendStreaks::default();
    let stop = |streaks: &SendStreaks| {
        streaks.ended() || cancel.is_cancelled() || std::time::Instant::now() >= deadline
    };
    let mut after: Option<(Uuid, Uuid)> = None;
    let mut sent = 0u32;
    while !stop(&streaks) {
        let due = claim_digest_due(pool, before, now, after).await?;
        // UPDATE .. RETURNING has no order: the walk resumes after the largest key.
        let Some(last) = due.iter().map(|(ws, user, _)| (*ws, *user)).max() else {
            break;
        };
        after = Some(last);
        let short = (due.len() as i64) < DIGEST_BATCH;

        let mut pending = due.into_iter();
        while let Some(claim) = pending.next() {
            if stop(&streaks) {
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
                    streaks = SendStreaks::default();
                }
                Ok(false) => {}
                Err(err) => {
                    // Source keeps lastDigestAt on failure so the window is
                    // retried. Hand the claim back at once: it lies behind
                    // the walk, so this sweep does not claim it again, and a
                    // sweep dropped later in the batch still leaves it due.
                    restore_claim(pool, workspace_id, user_id, prev_last, now).await?;
                    streaks.failed(&err);
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

/// Failed sends counted towards `DIGEST_DOWN_STREAK` and
/// `DIGEST_REFUSAL_STREAK` since the last sent digest or final refusal.
#[derive(Default)]
struct SendStreaks {
    down: u32,
    refused: u32,
}

impl SendStreaks {
    fn failed(&mut self, err: &DigestError) {
        match err {
            DigestError::Mail(mail) if smtp::is_final_for_recipient(&mail.code) => {
                *self = Self::default();
            }
            DigestError::Mail(mail) if smtp::is_unclassified_refusal(&mail.code) => {
                self.refused += 1;
            }
            _ => self.down += 1,
        }
    }

    fn ended(&self) -> bool {
        self.down >= DIGEST_DOWN_STREAK || self.refused >= DIGEST_REFUSAL_STREAK
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::MailSendError;

    fn mail(code: &str) -> DigestError {
        DigestError::Mail(MailSendError {
            op: "send",
            code: code.to_string(),
        })
    }

    #[test]
    fn a_5xx_counts_only_towards_the_refusal_streak() {
        let mut streaks = SendStreaks::default();
        for _ in 1..DIGEST_REFUSAL_STREAK {
            streaks.failed(&mail("permanent"));
        }
        assert!(!streaks.ended(), "a run of 5xx is not SMTP down");
        streaks.failed(&mail("permanent"));
        assert!(streaks.ended());
    }

    #[test]
    fn transport_failures_count_towards_the_down_streak() {
        let mut streaks = SendStreaks::default();
        for code in ["transient", "timeout", "connection", "tls_config"] {
            streaks.failed(&mail(code));
        }
        streaks.failed(&mail("permanent"));
        assert!(!streaks.ended(), "a 5xx neither adds to nor resets it");
        streaks.failed(&mail("transient"));
        assert!(streaks.ended());
    }

    #[test]
    fn a_final_refusal_resets_both_streaks() {
        let mut streaks = SendStreaks::default();
        for final_code in ["recipient_rejected", "invalid_recipient"] {
            for _ in 1..DIGEST_DOWN_STREAK {
                streaks.failed(&mail("transient"));
            }
            for _ in 1..DIGEST_REFUSAL_STREAK {
                streaks.failed(&mail("permanent"));
            }
            streaks.failed(&mail(final_code));
            streaks.failed(&mail("transient"));
            streaks.failed(&mail("permanent"));
            assert!(!streaks.ended(), "{final_code}");
            streaks = SendStreaks::default();
        }
    }
}
