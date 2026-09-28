use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::db::outbox::OutboxEvent;
use crate::mail::{smtp, Mailer};
use crate::notifications::{identity_mail_for_event, list_immediate_comment_mails, OutboundMail};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};

pub const MAIL_CONSUMER: &str = "mail";

const MAIL_VERBS: &[&str] = &["comment.created", "identity.linked", "identity.unlinked"];

/// Sends the mail of `comment.created` and `identity.*` events. The unit of
/// delivery is the recipient: a permanent refusal of the recipient's mailbox
/// is final for that recipient only, and a retry after any other failure
/// (including a relay-wide 5xx) skips the recipients SMTP already accepted
/// (see `AcceptedRecipients`).
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
/// first. The dispatcher sends one mail event at a time, so an event is
/// redelivered long before 64 newer mail events push it out.
const ACCEPTED_EVENTS_KEPT: usize = 64;

/// Recipients SMTP accepted, per event. An entry is kept after its event
/// completes too: the dispatcher marks the event processed only after
/// `deliver` returns, and when that mark (or the lease renewal next to it)
/// fails, the event is delivered again and must skip these recipients. An
/// entry is only ever a recipient SMTP really accepted, so keeping it can
/// never suppress a send that failed. This lives in memory only: a restart,
/// or another replica taking over the lease, starts empty and may send an
/// accepted recipient's mail again (the documented at-least-once edge, like
/// a crash between SMTP and the processed mark).
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

    /// One event per chunk, like the GitHub consumer. Sends are sequential,
    /// so batching gains nothing, and the default `deliver_batch` loses its
    /// progress when a chunk hits the lease timeout: every mail of the chunk
    /// would be sent again. With one event per chunk each event is marked
    /// processed right after its SMTP sends, and a timeout is charged to the
    /// event that overran.
    fn batch_event_cap(&self) -> usize {
        1
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { deliver_mail(pool, &self.mailer, &self.accepted, event).await })
    }
}

pub fn mail_consumer(mailer: Arc<Mailer>) -> Arc<dyn OutboxConsumer> {
    Arc::new(MailConsumer::new(mailer))
}

fn is_mail_verb(verb: &str) -> bool {
    MAIL_VERBS.contains(&verb)
}

async fn collect_mails(
    pool: &PgPool,
    event: &OutboxEvent,
) -> Result<Vec<OutboundMail>, OutboxProcessError> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    if let Some(workspace_id) = event.workspace_id {
        set_tenant(&mut tx, workspace_id).await?;
    }
    let mut mails = Vec::new();
    if event.verb == "comment.created" {
        mails.extend(list_immediate_comment_mails(&mut tx, event).await?);
    }
    if let Some(identity) = identity_mail_for_event(&mut tx, event).await? {
        mails.push(identity);
    }
    tx.commit().await?;
    Ok(mails)
}

async fn deliver_mail(
    pool: &PgPool,
    mailer: &Mailer,
    accepted: &Mutex<AcceptedRecipients>,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    if !is_mail_verb(&event.verb) {
        return Ok(());
    }
    let mails = collect_mails(pool, event).await?;
    let mut any_accepted = false;
    let mut rejected = 0usize;
    for mail in &mails {
        if lock(accepted).contains(event.id, &mail.to) {
            any_accepted = true;
            continue;
        }
        match mailer.send(&mail.to, &mail.subject, &mail.text).await {
            Ok(()) => {
                lock(accepted).insert(event.id, &mail.to);
                any_accepted = true;
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
            // May pass later, or refuses the whole relay: retry the event
            // (and dead-letter it where it is visible). The recipients
            // accepted so far are remembered and skipped on the retry.
            Err(err) => return Err(OutboxProcessError::Delivery(err.to_string())),
        }
    }
    if rejected > 0 && !any_accepted {
        // Nobody accepted, which is what a relay-wide refusal looks like: fail
        // so the event is retried and then dead-lettered where it is visible.
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
