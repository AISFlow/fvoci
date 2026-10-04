use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::backend::Backend;
use crate::db::outbox::{BackendOutboxEvent, OutboxEvent};
use crate::mail::{smtp, Mailer};
use crate::notifications::{
    identity_mail_for_event_backend, list_immediate_comment_mails_backend, OutboundMail,
};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};

pub const MAIL_CONSUMER: &str = "mail";

const MAIL_VERBS: &[&str] = &["comment.created", "identity.linked", "identity.unlinked"];

/// Sends the mail of `comment.created` and `identity.*` events, at least
/// once per recipient the relay accepts (see `AcceptedRecipients` for when
/// one gets it twice); a skipped address or a refused mailbox gets none.
///
/// The unit of delivery is the recipient. An address that does not parse as
/// a mailbox is skipped without asking the relay. A permanent refusal of the
/// recipient's mailbox is final for that recipient. Any other 5xx may refuse
/// one recipient (a policy refusal at `RCPT`) or every recipient (a relay
/// limit, a refused sender), so the send goes on to the next recipient and
/// the refusal becomes final for its recipient only once a later send in the
/// same attempt is accepted, which shows the relay still serves.
///
/// The event fails (it is retried, then dead-lettered where it is visible)
/// at once on a 4xx, timeout or connection failure; when such refusals are
/// left with no acceptance after them; and when the relay refused the
/// mailbox of every recipient it was asked about and none was accepted in
/// this attempt or an earlier one, as a relay that refuses everyone that
/// way would. The retry skips the recipients SMTP already accepted (see
/// `AcceptedRecipients`). A skipped recipient proves nothing about the relay
/// now, so a refusal the classifier cannot tie to the recipient (any 5xx
/// outside the X.1/X.2 mailbox codes, including a bare 550 or 553 from
/// relays that send no enhanced status codes, such as Exim by default or
/// qmail, for an unknown user) of the last recipient still to send looks
/// like a relay-wide one: that event retries and dead-letters, after the
/// recipients before it got their mail.
pub struct MailConsumer {
    mailer: Arc<Mailer>,
    accepted: Mutex<AcceptedRecipients>,
}

impl MailConsumer {
    pub fn new(mailer: Arc<Mailer>) -> Self {
        Self {
            mailer,
            accepted: Mutex::new(AcceptedRecipients::default()),
        }
    }
}

/// Events whose accepted recipients are kept at most, oldest pushed out
/// first. The consumer sends one mail event per call and the dispatcher
/// retries an event before it passes it, so an event is redelivered long
/// before 64 newer mail events push it out. A dead letter requeued by hand
/// (`fvoci.app_outbox_requeue`, SQL only) is the exception: it is delivered
/// again after the cursor passed it, possibly after more newer mail events,
/// and its accepted recipients may then get the mail again.
const ACCEPTED_EVENTS_KEPT: usize = 64;

/// Recipients SMTP accepted, per event. An entry is kept after its event
/// completes too: the dispatcher marks the event processed only after
/// `deliver` returns, and when that mark fails, the event is delivered again
/// and must skip these recipients. An entry is only ever a recipient SMTP
/// really accepted, so keeping it can never suppress a send that failed.
///
/// A recipient gets the mail twice when its entry is missing: a send whose
/// acceptance was not seen (the session timeout, a connection dropped after
/// DATA, or the dispatcher dropping the call at its lease timeout) is not
/// recorded and the retry sends it again; and the entries live in memory
/// only, so a restart, or another replica taking over the lease, starts
/// empty and may send an accepted recipient's mail again.
#[derive(Default)]
struct AcceptedRecipients {
    events: VecDeque<(Uuid, HashSet<String>)>,
}

impl AcceptedRecipients {
    fn contains(&self, event_id: Uuid, to: &str) -> bool {
        self.events
            .iter()
            .any(|(id, accepted)| *id == event_id && accepted.contains(to))
    }

    fn insert(&mut self, event_id: Uuid, to: &str) {
        if let Some((_, accepted)) = self.events.iter_mut().find(|(id, _)| *id == event_id) {
            accepted.insert(to.to_string());
            return;
        }
        if self.events.len() >= ACCEPTED_EVENTS_KEPT {
            self.events.pop_front();
        }
        self.events
            .push_back((event_id, HashSet::from([to.to_string()])));
    }
}

fn lock(accepted: &Mutex<AcceptedRecipients>) -> MutexGuard<'_, AcceptedRecipients> {
    accepted.lock().unwrap_or_else(PoisonError::into_inner)
}

impl OutboxConsumer for MailConsumer {
    fn name(&self) -> &str {
        MAIL_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { deliver_mail(pool, &self.mailer, &self.accepted, event).await })
    }

    /// A mail event alone, or the run of events without mail up to the next
    /// mail event (no I/O: they only need their processed mark). The
    /// dispatcher drops a call that runs past the lease timeout with all its
    /// progress and charges the failure to the call's first event, so a call
    /// holds at most one mail event and never one behind other events: a
    /// timeout then drops only that event's progress (its accepted
    /// recipients are remembered) and is charged to the event that overran.
    fn deliver_batch<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        events: &'a [OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            let Some(first) = events.first() else {
                return (0, None);
            };
            if is_mail_verb(&first.verb) {
                return match deliver_mail(pool, &self.mailer, &self.accepted, first).await {
                    Ok(()) => (1, None),
                    Err(err) => (0, Some(err)),
                };
            }
            let run = events
                .iter()
                .take_while(|event| !is_mail_verb(&event.verb))
                .count();
            (run, None)
        })
    }
    fn deliver_backend<'a>(
        &'a self,
        backend: &'a Backend,
        _owner: Uuid,
        event: &'a BackendOutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(
            async move { deliver_mail_backend(backend, &self.mailer, &self.accepted, event).await },
        )
    }
    fn deliver_batch_backend<'a>(
        &'a self,
        backend: &'a Backend,
        _owner: Uuid,
        events: &'a [BackendOutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            let Some(first) = events.first() else {
                return (0, None);
            };
            if is_mail_verb(&first.verb) {
                return match deliver_mail_backend(backend, &self.mailer, &self.accepted, first)
                    .await
                {
                    Ok(()) => (1, None),
                    Err(err) => (0, Some(err)),
                };
            }
            (
                events.iter().take_while(|e| !is_mail_verb(&e.verb)).count(),
                None,
            )
        })
    }
}

pub fn mail_consumer(mailer: Arc<Mailer>) -> Arc<dyn OutboxConsumer> {
    Arc::new(MailConsumer::new(mailer))
}

fn is_mail_verb(verb: &str) -> bool {
    MAIL_VERBS.contains(&verb)
}

async fn collect_mails_backend(
    backend: &Backend,
    event: &BackendOutboxEvent,
) -> Result<Vec<OutboundMail>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let previous = tx.operation().set_system().await?;
    if let Some(workspace_id) = event.workspace_id {
        tx.operation().set_tenant(workspace_id).await?;
    }
    let mut mails = list_immediate_comment_mails_backend(&mut tx.operation(), event).await?;
    if let Some(mail) = identity_mail_for_event_backend(&mut tx.operation(), event).await? {
        mails.push(mail);
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(mails)
}

async fn deliver_mail(
    pool: &PgPool,
    mailer: &Mailer,
    accepted: &Mutex<AcceptedRecipients>,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    deliver_mail_backend(
        &Backend::Postgres(pool.clone()),
        mailer,
        accepted,
        &event.clone().into(),
    )
    .await
}

async fn deliver_mail_backend(
    backend: &Backend,
    mailer: &Mailer,
    accepted: &Mutex<AcceptedRecipients>,
    event: &BackendOutboxEvent,
) -> Result<(), OutboxProcessError> {
    if !is_mail_verb(&event.verb) {
        return Ok(());
    }
    let mails = collect_mails_backend(backend, event).await?;
    let mut any_accepted = false;
    let mut rejected = 0usize;
    // 5xx refusals not known to be about their recipient that no send
    // accepted in this attempt has followed yet.
    let mut unproven = 0usize;
    for mail in &mails {
        if lock(accepted).contains(event.id, &mail.to) {
            // Accepted by an earlier attempt. That says nothing about the
            // relay now, so it does not prove the refusals before it.
            any_accepted = true;
            continue;
        }
        match mailer.send(&mail.to, &mail.subject, &mail.text).await {
            Ok(()) => {
                lock(accepted).insert(event.id, &mail.to);
                any_accepted = true;
                // The relay accepts after those refusals, so it is not
                // refusing everyone: they were final for their recipients.
                rejected += unproven;
                unproven = 0;
            }
            Err(err) if smtp::is_unsendable_address(&err.code) => {
                // The relay was never asked and no retry can send it: skip
                // the recipient. It proves nothing about the relay, so it
                // counts neither as refused nor as accepted. The error
                // carries no address.
                tracing::warn!(
                    event_id = %event.id,
                    code = %err.code,
                    "mail.recipient_rejected"
                );
            }
            Err(err) if smtp::is_final_for_recipient(&err.code) => {
                // Final for this recipient only. The error carries no address.
                rejected += 1;
                tracing::warn!(
                    event_id = %event.id,
                    code = %err.code,
                    "mail.recipient_rejected"
                );
            }
            Err(err) if smtp::is_unclassified_refusal(&err.code) => {
                // One recipient or the whole relay: a later accepted send
                // decides. The error carries no address or server text.
                unproven += 1;
                tracing::warn!(
                    event_id = %event.id,
                    code = %err.code,
                    "mail.recipient_refused_unclassified"
                );
            }
            // May pass later: retry the event (and dead-letter it where it
            // is visible). Stopping here bounds the attempt; the recipients
            // accepted so far are remembered and skipped on the retry.
            Err(err) => return Err(OutboxProcessError::Delivery(err.to_string())),
        }
    }
    if unproven > 0 {
        // No send was accepted after these refusals, which is what a
        // relay-wide refusal (such as a daily limit that starts partway
        // through) looks like: fail so the event is retried and then
        // dead-lettered where it is visible.
        return Err(OutboxProcessError::Delivery(format!(
            "mailer: {unproven} recipients refused with no accepted send after them"
        )));
    }
    if rejected > 0 && !any_accepted {
        // The relay refused the mailbox of every recipient it was asked
        // about and accepted none: the same relay-wide case as above.
        return Err(OutboxProcessError::Delivery(format!(
            "mailer: every recipient rejected ({rejected})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_recipients_are_per_event_and_bounded() {
        let mut accepted = AcceptedRecipients::default();
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        accepted.insert(first, "a@example.com");
        accepted.insert(first, "b@example.com");
        accepted.insert(second, "a@example.com");
        assert!(accepted.contains(first, "a@example.com"));
        assert!(accepted.contains(first, "b@example.com"));
        assert!(!accepted.contains(second, "b@example.com"));

        for _ in 0..ACCEPTED_EVENTS_KEPT - 1 {
            accepted.insert(Uuid::now_v7(), "c@example.com");
        }
        assert_eq!(accepted.events.len(), ACCEPTED_EVENTS_KEPT);
        assert!(
            !accepted.contains(first, "a@example.com"),
            "the oldest event is pushed out"
        );
        assert!(accepted.contains(second, "a@example.com"));
    }
}

#[cfg(test)]
mod backend_delivery_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::mail::SmtpConfig;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn backend_batch_confirms_only_nonmail_prefix_or_one_mail_event() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        let event = f.append_comment_event("comment.created").await;
        let consumer = MailConsumer::new(Arc::new(Mailer::disabled()));
        let mut wiki = event.clone();
        wiki.verb = "document.updated".into();
        let owner = Uuid::now_v7();
        let (done, error) = consumer
            .deliver_batch_backend(
                &f.backend,
                owner,
                &[wiki.clone(), wiki.clone(), event.clone(), wiki.clone()],
            )
            .await;
        assert_eq!(done, 2);
        assert!(error.is_none());
        let (done, error) = consumer
            .deliver_batch_backend(&f.backend, owner, &[event.clone(), event.clone(), wiki])
            .await;
        assert_eq!(done, 1);
        assert!(error.is_none());
        assert_eq!(
            lock(&consumer.accepted).events.len(),
            1,
            "unset SMTP preserves the actual configured no-op"
        );
        let marks: i64 =
            sqlx::query_scalar("SELECT count(*) FROM processed_events WHERE consumer='mail'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            marks, 0,
            "External consumer cannot mark or advance before dispatcher confirmation"
        );
        f.finish().await;
    }

    // A synthetic local SMTP peer: no TLS/AUTH advertised, no real address or
    // external endpoint. Only the existing transport sends the actual message.
    async fn smtp_sink(
        listener: tokio::net::TcpListener,
        accepted: Arc<Mutex<Vec<String>>>,
        mut drop_first_confirmation: bool,
    ) {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let accepted = accepted.clone();
            {
                let mut stream = BufReader::new(stream);
                stream
                    .get_mut()
                    .write_all(b"220 fixture ESMTP\r\n")
                    .await
                    .unwrap();
                let mut line = String::new();
                let mut data = false;
                let mut recipient = String::new();
                let mut body = String::new();
                loop {
                    line.clear();
                    if stream.read_line(&mut line).await.unwrap() == 0 {
                        break;
                    }
                    if data {
                        if line == ".\r\n" {
                            assert!(body.contains("Subject:"));
                            accepted.lock().unwrap().push(recipient.clone());
                            if drop_first_confirmation {
                                drop_first_confirmation = false;
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
                    } else if upper.starts_with("RCPT TO:") {
                        recipient = line.trim().to_string();
                        "250 2.1.5 recipient\r\n"
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
        }
    }

    #[tokio::test]
    async fn backend_actual_smtp_confirmation_cache_and_restart_are_at_least_once() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        let event = f.append_comment_event("comment.created").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = tokio::spawn(smtp_sink(listener, received.clone(), false));
        let mailer = Arc::new(Mailer::from_smtp(Some(SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            from: "sender@notification.invalid".into(),
        })));
        let consumer = MailConsumer::new(mailer.clone());
        let owner = Uuid::now_v7();
        consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "actual SMTP DATA acceptance is required"
        );
        consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(
            received.lock().unwrap().len(),
            1,
            "same-process replay skips accepted recipient"
        );
        let restarted = MailConsumer::new(mailer);
        restarted
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(
            received.lock().unwrap().len(),
            2,
            "restart permits documented at-least-once redelivery"
        );
        sink.abort();
        assert!(sink.await.unwrap_err().is_cancelled());
        f.finish().await;
    }
    #[tokio::test]
    async fn backend_smtp_unknown_response_retries_without_false_confirmation() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        let event = f.append_comment_event("comment.created").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = tokio::spawn(smtp_sink(listener, received.clone(), true));
        let mailer = Arc::new(Mailer::from_smtp(Some(SmtpConfig {
            host: "127.0.0.1".into(),
            port,
            from: "sender@notification.invalid".into(),
        })));
        let consumer = MailConsumer::new(mailer);
        let owner = Uuid::now_v7();
        let (done, error) = consumer
            .deliver_batch_backend(&f.backend, owner, std::slice::from_ref(&event))
            .await;
        assert_eq!(
            done, 0,
            "DATA without a received acceptance cannot confirm the event"
        );
        assert!(error.is_some());
        assert_eq!(received.lock().unwrap().len(), 1);
        assert!(lock(&consumer.accepted).events.is_empty());
        let (done, error) = consumer
            .deliver_batch_backend(&f.backend, owner, std::slice::from_ref(&event))
            .await;
        assert_eq!(done, 1);
        assert!(error.is_none());
        assert_eq!(
            received.lock().unwrap().len(),
            2,
            "unknown acceptance permits at-least-once redelivery"
        );
        consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(
            received.lock().unwrap().len(),
            2,
            "only the confirmed retry is cached"
        );
        sink.abort();
        assert!(sink.await.unwrap_err().is_cancelled());
        f.finish().await;
    }
}
