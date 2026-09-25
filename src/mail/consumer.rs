use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::db::outbox::OutboxEvent;
use crate::mail::Mailer;
use crate::notifications::{identity_mail_for_event, list_immediate_comment_mails, OutboundMail};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};

pub const MAIL_CONSUMER: &str = "mail";

const MAIL_VERBS: &[&str] = &["comment.created", "identity.linked", "identity.unlinked"];

pub struct MailConsumer {
    mailer: Arc<Mailer>,
}

impl MailConsumer {
    pub fn new(mailer: Arc<Mailer>) -> Self {
        Self { mailer }
    }
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
        Box::pin(async move { deliver_mail(pool, &self.mailer, event).await })
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
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    if !is_mail_verb(&event.verb) {
        return Ok(());
    }
    let mails = collect_mails(pool, event).await?;
    for mail in mails {
        mailer
            .send(&mail.to, &mail.subject, &mail.text)
            .await
            .map_err(|err| OutboxProcessError::Delivery(err.to_string()))?;
    }
    Ok(())
}
