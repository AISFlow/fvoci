use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::backend::Backend;
use crate::db::mail_digest::DigestClaim;
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
    send_due_digests_backend(&Backend::Postgres(pool.clone()), mailer, now, cancel).await
}

/// Same daily scheduling and aggregate notification window on the selected
/// backend. Scheduler clock values enter storage at signed UTC microseconds.
pub async fn send_due_digests_backend(
    backend: &Backend,
    mailer: &Mailer,
    now: DateTime<Utc>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<u32, DigestError> {
    let now = DateTime::from_timestamp_micros(now.timestamp_micros())
        .ok_or_else(|| sqlx::Error::Protocol("digest scheduler instant out of range".into()))?;
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
        let due = claim_digest_due(backend, before, now, after).await?;
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
                    restore_claim(backend, workspace_id, user_id, prev_last, now).await?;
                }
                break;
            }
            let (workspace_id, user_id, prev_last) = claim;
            match send_claimed(backend, mailer, workspace_id, user_id, prev_last, now).await {
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
                    restore_claim(backend, workspace_id, user_id, prev_last, now).await?;
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
    backend: &Backend,
    workspace_id: Uuid,
    user_id: Uuid,
    prev_last: Option<DateTime<Utc>>,
    claimed_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    tx.operation().set_tenant(workspace_id).await?;
    tx.operation()
        .digest_restore_claim(workspace_id, user_id, prev_last, claimed_at)
        .await?;
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(())
}

async fn claim_digest_due(
    backend: &Backend,
    before: DateTime<Utc>,
    now: DateTime<Utc>,
    after: Option<(Uuid, Uuid)>,
) -> Result<Vec<DigestClaim>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    let claims = tx
        .operation()
        .digest_claim_due(before, now, after, DIGEST_BATCH)
        .await?;
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(claims)
}

async fn send_claimed(
    backend: &Backend,
    mailer: &Mailer,
    workspace: Uuid,
    user: Uuid,
    prev_last: Option<DateTime<Utc>>,
    claimed_at: DateTime<Utc>,
) -> Result<bool, DigestError> {
    let packed = {
        let mut tx = backend.begin_read().await?;
        let previous = tx.operation().set_system().await?;
        tx.operation().set_tenant(workspace).await?;
        let packed = match tx
            .operation()
            .digest_recipient(workspace, user, claimed_at)
            .await?
        {
            Some(email) => Some((
                email,
                tx.operation()
                    .digest_unread_count(workspace, user, prev_last)
                    .await?,
            )),
            None => None,
        };
        tx.operation().restore_system(previous).await?;
        tx.commit()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
        packed
    };
    let Some((email, count)) = packed else {
        return Ok(false);
    };
    if count <= 0 {
        return Ok(false);
    };
    if !mailer.enabled() {
        // Unconfigured SMTP did not send: return the actual fenced claim.
        restore_claim(backend, workspace, user, prev_last, claimed_at).await?;
        return Ok(false);
    }
    mailer
        .send(&email, DIGEST_SUBJECT, &digest_text(count))
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

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::mail::SmtpConfig;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio_util::sync::CancellationToken;

    fn clock() -> DateTime<Utc> {
        DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
    }
    async fn ready(f: &Fixture) {
        f.grant_wiki().await;
        crate::db::outbox::ensure_consumer_backend(
            &f.backend,
            crate::notifications::NOTIFICATIONS_CONSUMER,
        )
        .await
        .unwrap();
        let owner = Uuid::now_v7();
        assert!(crate::db::outbox::lease_consumer_backend(
            &f.backend,
            crate::notifications::NOTIFICATIONS_CONSUMER,
            owner,
            60
        )
        .await
        .unwrap());
        let event = f.append_comment_event("comment.created").await;
        crate::notifications::process_notification_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO notification_prefs(workspace_id,user_id,mail_digest) VALUES(?1,?2,1)",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    }
    async fn last(f: &Fixture) -> Option<i64> {
        sqlx::query_scalar(
            "SELECT last_digest_at FROM notification_prefs WHERE workspace_id=?1 AND user_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap()
    }
    struct Sink {
        mails: Arc<Mutex<Vec<String>>>,
        job: tokio::task::JoinHandle<()>,
        mailer: Mailer,
    }
    impl Sink {
        async fn new(mut drop_first: bool) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let mails = Arc::new(Mutex::new(Vec::new()));
            let captured = mails.clone();
            let job = tokio::spawn(async move {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut stream = BufReader::new(stream);
                    stream
                        .get_mut()
                        .write_all(b"220 fixture ESMTP\r\n")
                        .await
                        .unwrap();
                    let mut line = String::new();
                    let mut data = false;
                    let mut body = String::new();
                    loop {
                        line.clear();
                        if stream.read_line(&mut line).await.unwrap() == 0 {
                            break;
                        }
                        if data {
                            if line == ".\r\n" {
                                captured.lock().unwrap().push(body.clone());
                                if drop_first {
                                    drop_first = false;
                                    break;
                                }
                                data = false;
                                stream
                                    .get_mut()
                                    .write_all(b"250 2.0.0 accepted\r\n")
                                    .await
                                    .unwrap();
                            } else {
                                body.push_str(&line);
                            }
                            continue;
                        }
                        let upper = line.to_ascii_uppercase();
                        let reply = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                            "250 fixture\r\n"
                        } else if upper == "DATA\r\n" {
                            data = true;
                            body.clear();
                            "354 data\r\n"
                        } else if upper == "QUIT\r\n" {
                            stream.get_mut().write_all(b"221 bye\r\n").await.unwrap();
                            break;
                        } else {
                            "250 ok\r\n"
                        };
                        stream.get_mut().write_all(reply.as_bytes()).await.unwrap();
                    }
                }
            });
            Self {
                mails,
                job,
                mailer: Mailer::from_smtp(Some(SmtpConfig {
                    host: "127.0.0.1".into(),
                    port,
                    from: "sender@digest.invalid".into(),
                })),
            }
        }
        async fn finish(self) {
            self.job.abort();
            assert!(self.job.await.unwrap_err().is_cancelled());
        }
    }

    #[tokio::test]
    async fn actual_backend_digest_sends_aggregate_once_across_runners_and_keeps_target_private() {
        let f = Fixture::new().await;
        ready(&f).await;
        let sink = Sink::new(false).await;
        let now = clock();
        // A retained unread notification still counts, but no target data is sent.
        sqlx::query(
            "UPDATE documents SET title='protected-title-never-loaded',deleted_at=?1 WHERE id=?2",
        )
        .bind(now.timestamp_micros())
        .bind(f.document.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE notifications SET payload=?1")
            .bind(serde_json::json!({"body":"protected-body-never-loaded"}).to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        let other = Backend::Sqlite(
            crate::db::pool::connect_sqlite_app(&f.dir.join("test.sqlite"), 1)
                .await
                .unwrap(),
        );
        let cancel = CancellationToken::new();
        let (a, b) = tokio::join!(
            send_due_digests_backend(&f.backend, &sink.mailer, now, &cancel),
            send_due_digests_backend(&other, &sink.mailer, now, &cancel)
        );
        assert_eq!(a.unwrap() + b.unwrap(), 1);
        assert_eq!(last(&f).await, Some(now.timestamp_micros()));
        assert_eq!(
            send_due_digests_backend(&f.backend, &sink.mailer, now, &cancel)
                .await
                .unwrap(),
            0
        );
        let messages = sink.mails.lock().unwrap().clone();
        assert_eq!(messages.len(), 1);
        let parsed = mailparse::parse_mail(messages[0].as_bytes()).unwrap();
        assert_eq!(parsed.get_body().unwrap().trim(), digest_text(1));
        assert!(!messages[0].contains("protected-title"));
        assert!(!messages[0].contains("protected-body"));
        assert!(!messages[0].contains(&f.document.to_string()));
        other.close().await.unwrap();
        sink.finish().await;
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_backend_absent_smtp_restores_unsent_claim_and_previous_window() {
        let f = Fixture::new().await;
        ready(&f).await;
        let now = clock();
        assert_eq!(
            send_due_digests_backend(
                &f.backend,
                &Mailer::disabled(),
                now,
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            last(&f).await,
            None,
            "no configured SMTP must not consume the real due claim"
        );
        let previous = now - chrono::Duration::days(2) + chrono::Duration::microseconds(7);
        sqlx::query("UPDATE notification_prefs SET last_digest_at=?1")
            .bind(previous.timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            send_due_digests_backend(
                &f.backend,
                &Mailer::disabled(),
                now,
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(last(&f).await, Some(previous.timestamp_micros()));
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_backend_unknown_response_restores_window_and_restart_redelivers() {
        let f = Fixture::new().await;
        ready(&f).await;
        let sink = Sink::new(true).await;
        let now = clock();
        assert_eq!(
            send_due_digests_backend(&f.backend, &sink.mailer, now, &CancellationToken::new())
                .await
                .unwrap(),
            0
        );
        assert_eq!(last(&f).await, None);
        assert_eq!(sink.mails.lock().unwrap().len(), 1);
        assert_eq!(
            send_due_digests_backend(&f.backend, &sink.mailer, now, &CancellationToken::new())
                .await
                .unwrap(),
            1,
            "restart/sweep uses restored original count window"
        );
        assert_eq!(sink.mails.lock().unwrap().len(), 2);
        assert_eq!(last(&f).await, Some(now.timestamp_micros()));
        sink.finish().await;
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_backend_current_recipient_authority_and_claim_fence_prevent_sends() {
        let f = Fixture::new().await;
        ready(&f).await;
        let sink = Sink::new(false).await;
        let now = clock();
        let due = claim_digest_due(&f.backend, now - chrono::Duration::days(1), now, None)
            .await
            .unwrap();
        assert_eq!(due, vec![(f.workspace, f.user, None)]);
        assert!(
            send_claimed(&f.backend, &sink.mailer, f.workspace, f.user, None, now)
                .await
                .unwrap()
        );
        assert_eq!(sink.mails.lock().unwrap().len(), 1);
        assert!(!send_claimed(
            &f.backend,
            &sink.mailer,
            f.workspace,
            f.user,
            None,
            now - chrono::Duration::microseconds(1)
        )
        .await
        .unwrap());
        assert_eq!(last(&f).await, Some(now.timestamp_micros()));
        sqlx::query("UPDATE notification_prefs SET mail_digest=0")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !send_claimed(&f.backend, &sink.mailer, f.workspace, f.user, None, now)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE notification_prefs SET mail_digest=1")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET deleted_at=?1 WHERE id=?2")
            .bind(now.timestamp_micros())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !send_claimed(&f.backend, &sink.mailer, f.workspace, f.user, None, now)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE users SET deleted_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=?1 WHERE id=?2")
            .bind(now.timestamp_micros())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !send_claimed(&f.backend, &sink.mailer, f.workspace, f.user, None, now)
                .await
                .unwrap()
        );
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            !send_claimed(&f.backend, &sink.mailer, f.workspace, f.user, None, now)
                .await
                .unwrap()
        );
        assert_eq!(
            sink.mails.lock().unwrap().len(),
            1,
            "no current-authority denial can send, even with enabled transport"
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notification_prefs WHERE workspace_id=?1 AND user_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            count, 0,
            "lost membership cannot recreate claim/preferences"
        );
        sink.finish().await;
        f.finish().await;
    }
}
