//! `push` outbox consumer: fan-out only (source `listPushMessages`); sending
//! is [`crate::push::sender`], the same split as webhooks.
//!
//! In one transaction with its own processed mark and cursor advance
//! (independent of `notifications` and `mail`), each event becomes one
//! `push_deliveries` row per (recipient, subscription). Recipients are the
//! `notify_for_event` rows whose `inApp` pref is on and whose user is active.
//! Only subscriptions bound to a live session are queued. The sender checks
//! all of this again right before handing a row off, so the fan-out only has
//! to be complete, not final.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::notifications::{resolved_store_prefs, NotificationInsert};
use crate::db::outbox::{
    advance_cursor_backend_tx, mark_processed_backend_tx, BackendOutboxEvent, OutboxEvent,
};
use crate::notifications::notify_for_event_backend;
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::push::message::{format_person_name, notification_body, push_url};
use crate::push::send::PushPayload;

pub const PUSH_CONSUMER: &str = "push";

pub struct PushConsumer {
    /// Wakes the sender after a fan-out commit.
    sender_wake: Option<Arc<Notify>>,
}

impl OutboxConsumer for PushConsumer {
    fn name(&self) -> &str {
        PUSH_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::DatabaseAtomic
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            self.deliver_backend(
                &Backend::Postgres(pool.clone()),
                lease_owner,
                &event.clone().into(),
            )
            .await
        })
    }
    fn deliver_backend<'a>(
        &'a self,
        backend: &'a Backend,
        lease_owner: Uuid,
        event: &'a BackendOutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let queued = fan_out_event_backend(backend, lease_owner, event).await?;
            // Never wake on rejected advancement, failed/unknown commit, or a
            // duplicate/no-recipient event. Durable queue state comes first.
            if queued > 0 {
                if let Some(wake) = &self.sender_wake {
                    wake.notify_one();
                }
            }
            Ok(())
        })
    }
}

pub fn push_consumer(sender_wake: Option<Arc<Notify>>) -> Arc<dyn OutboxConsumer> {
    Arc::new(PushConsumer { sender_wake })
}

async fn fan_out_event_backend(
    backend: &Backend,
    lease_owner: Uuid,
    event: &BackendOutboxEvent,
) -> Result<u64, OutboxProcessError> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    if let Some(workspace) = event.workspace_id {
        tx.operation().set_tenant(workspace).await?;
    }
    let mut queued = 0;
    if mark_processed_backend_tx(&mut tx, PUSH_CONSUMER, event.id).await? {
        queued = fan_out_backend(&mut tx.operation(), event).await?;
    }
    if !advance_cursor_backend_tx(&mut tx, PUSH_CONSUMER, lease_owner, &event.cursor()).await? {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in push transaction".into(),
        ));
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| OutboxProcessError::Db(sqlx::Error::AnyDriverError(Box::new(e))))?;
    Ok(queued)
}

async fn fan_out_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
) -> Result<u64, sqlx::Error> {
    let Some(workspace) = event.workspace_id else {
        return Ok(0);
    };
    if tx.read_vapid_public_key().await?.is_none() {
        return Ok(0);
    }
    let recipients = push_recipients_backend(tx, event).await?;
    let mut users: Vec<Uuid> = recipients.keys().copied().collect();
    users.sort_unstable();
    tx.enqueue_push_deliveries(event.id, workspace, &users)
        .await
}

/// Current push recipients of `event`, keyed by user: the in-app notification
/// rows (membership, resource permission and actor rules of
/// `notify_for_event`) with `inApp` on, for users who are neither deleted nor
/// suspended. The caller's transaction must have the event's tenant set.
pub(crate) async fn push_recipients(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<HashMap<Uuid, NotificationInsert>, sqlx::Error> {
    push_recipients_backend(&mut OperationTx::Postgres(tx), &event.clone().into()).await
}

pub(crate) async fn push_recipients_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
) -> Result<HashMap<Uuid, NotificationInsert>, sqlx::Error> {
    let Some(workspace) = event.workspace_id else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for row in notify_for_event_backend(tx, event).await? {
        let prefs = resolved_store_prefs(tx.notification_prefs(workspace, row.user_id).await?);
        if prefs.in_app && tx.push_user_is_active(row.user_id).await? {
            out.insert(row.user_id, row);
        }
    }
    Ok(out)
}

/// Source `listPushMessages` payload for one notification row.
pub(crate) async fn push_payload(
    tx: &mut Transaction<'_, Postgres>,
    row: &NotificationInsert,
) -> Result<Option<PushPayload>, sqlx::Error> {
    push_payload_backend(&mut OperationTx::Postgres(tx), row).await
}

pub(crate) async fn push_payload_backend(
    tx: &mut OperationTx<'_, '_>,
    row: &NotificationInsert,
) -> Result<Option<PushPayload>, sqlx::Error> {
    let Some((slug, workspace_name)) = tx.push_workspace_label(row.workspace_id).await? else {
        return Ok(None);
    };
    let display_id = tx
        .notification_display_id(
            row.workspace_id,
            row.target_type.as_deref(),
            row.target_id,
            &row.payload,
        )
        .await?;
    let actor = if let Some(id) = row.actor_user_id {
        tx.push_actor_label(id).await?
    } else {
        None
    };
    let title = actor
        .map(|(given, family, locale)| format_person_name(&given, family.as_deref(), &locale))
        .filter(|name| !name.is_empty())
        .unwrap_or(workspace_name);
    Ok(Some(PushPayload {
        title,
        body: notification_body(&row.verb, &row.payload),
        url: push_url(&slug, display_id.as_deref()),
    }))
}

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::db::outbox::{
        ensure_consumer_backend, fetch_cursor_backend, lease_consumer_backend, OutboxCursor,
    };
    use futures_util::FutureExt;

    async fn prepare(f: &Fixture, key: bool) -> Uuid {
        f.grant_wiki().await;
        if key {
            // Only the public field is needed by fan-out; no key generation,
            // private-material load, external service or sender invocation.
            sqlx::query("UPDATE instance_config SET vapid_public_key='fixture-public'")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        ensure_consumer_backend(&f.backend, PUSH_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(lease_consumer_backend(&f.backend, PUSH_CONSUMER, owner, 60)
            .await
            .unwrap());
        owner
    }

    async fn subscription(f: &Fixture, user: Uuid, session: Uuid, index: usize) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO push_subscriptions(id,user_id,session_id,endpoint,p256dh,auth) VALUES(?1,?2,?3,?4,?5,?6)")
            .bind(id.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(session.as_bytes().as_slice())
            .bind(format!("https://push.example.invalid/fixture-{index}")).bind(format!("B{}","A".repeat(86))).bind("A".repeat(22))
            .execute(&f.pool).await.unwrap();
        id
    }

    async fn counts(f: &Fixture, event: Uuid) -> (i64, i64) {
        let effects = sqlx::query_scalar("SELECT COUNT(*) FROM push_deliveries WHERE event_id=?1")
            .bind(event.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let marker = sqlx::query_scalar(
            "SELECT COUNT(*) FROM processed_events WHERE consumer='push' AND event_id=?1",
        )
        .bind(event.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        (effects, marker)
    }

    async fn cursor(f: &Fixture, expected: i64) {
        assert!(
            matches!(fetch_cursor_backend(&f.backend,PUSH_CONSUMER).await.unwrap(),
            Some(OutboxCursor::SqliteFamily{seq}) if seq==expected)
        );
    }

    #[tokio::test]
    async fn actual_queue_marker_cursor_wrong_lease_insert_failure_duplicate_and_postcommit_wake() {
        let f = Fixture::new().await;
        let owner = prepare(&f, true).await;
        subscription(&f, f.user, f.credential, 0).await;
        subscription(&f, f.user, f.credential, 1).await;
        subscription(&f, f.other_user, f.other_credential, 0).await;
        let event = f.append_comment_event("comment.created").await;
        let wake = Arc::new(Notify::new());
        let consumer = push_consumer(Some(wake.clone()));
        assert_eq!(consumer.delivery_mode(), DeliveryMode::DatabaseAtomic);
        assert!(consumer
            .deliver_backend(&f.backend, Uuid::now_v7(), &event)
            .await
            .is_err());
        assert_eq!(counts(&f, event.id).await, (0, 0));
        cursor(&f, 0).await;
        assert!(wake.notified().now_or_never().is_none());
        sqlx::raw_sql("CREATE TRIGGER reject_push BEFORE INSERT ON push_deliveries BEGIN SELECT RAISE(ABORT,'injected push failure'); END;")
            .execute(&f.pool).await.unwrap();
        assert!(consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .is_err());
        assert_eq!(counts(&f, event.id).await, (0, 0));
        cursor(&f, 0).await;
        assert!(wake.notified().now_or_never().is_none());
        sqlx::query("DROP TRIGGER reject_push")
            .execute(&f.pool)
            .await
            .unwrap();
        consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        // The successful wake permit is examined only after fresh committed
        // reads show its queue, marker and actual event-sequence cursor.
        assert_eq!(counts(&f, event.id).await, (2, 1));
        cursor(&f, event.seq).await;
        assert!(wake.notified().now_or_never().is_some());
        let bindings: Vec<Vec<u8>> =
            sqlx::query_scalar("SELECT user_id FROM push_deliveries WHERE event_id=?1")
                .bind(event.id.as_bytes().as_slice())
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert!(bindings.iter().all(|id| id.as_slice() == f.user.as_bytes()));
        assert!(consumer
            .deliver_backend(&f.backend, owner, &event)
            .await
            .is_err());
        assert_eq!(counts(&f, event.id).await, (2, 1));
        assert!(wake.notified().now_or_never().is_none());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_subscriptions")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            3,
            "fan-out must preserve other-account subscriptions sharing an endpoint"
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn no_public_key_or_no_recipients_still_atomically_advances_marker_and_cursor() {
        for key in [false, true] {
            let f = Fixture::new().await;
            let owner = prepare(&f, key).await;
            subscription(&f, f.user, f.credential, 0).await;
            let mut event = f.append_comment_event("comment.created").await;
            if key {
                sqlx::query("UPDATE events SET verb='document.created' WHERE id=?1")
                    .bind(event.id.as_bytes().as_slice())
                    .execute(&f.pool)
                    .await
                    .unwrap();
                event = crate::db::outbox::fetch_event_by_id_backend(&f.backend, event.id)
                    .await
                    .unwrap()
                    .unwrap();
            }
            let wake = Arc::new(Notify::new());
            push_consumer(Some(wake.clone()))
                .deliver_backend(&f.backend, owner, &event)
                .await
                .unwrap();
            assert_eq!(counts(&f, event.id).await, (0, 1));
            cursor(&f, event.seq).await;
            assert!(wake.notified().now_or_never().is_none());
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn current_target_and_membership_revocation_deny_both_comment_verbs() {
        for membership in [false, true] {
            let f = Fixture::new().await;
            let owner = prepare(&f, true).await;
            subscription(&f, f.user, f.credential, 0).await;
            let query = if membership {
                "DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2"
            } else {
                "DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2"
            };
            sqlx::query(query)
                .bind(f.workspace.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            for verb in ["comment.created", "comment.resolved"] {
                let event = f.append_comment_event(verb).await;
                push_consumer(None)
                    .deliver_backend(&f.backend, owner, &event)
                    .await
                    .unwrap();
                assert_eq!(counts(&f, event.id).await, (0, 1));
                cursor(&f, event.seq).await;
            }
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn preferences_active_accounts_and_current_session_expiry_filter_real_queue() {
        for denied in [
            "prefs",
            "suspended",
            "deleted",
            "revoked",
            "expired",
            "foreign_session",
        ] {
            let f = Fixture::new().await;
            let owner = prepare(&f, true).await;
            let session = if denied == "foreign_session" {
                f.other_credential
            } else {
                f.credential
            };
            subscription(&f, f.user, session, 0).await;
            match denied {
                "prefs" => {
                    sqlx::query("INSERT INTO notification_prefs(workspace_id,user_id,in_app,mail_immediate,mail_digest) VALUES(?1,?2,0,1,1)")
                    .bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
                }
                "suspended" | "deleted" => {
                    let query = if denied == "suspended" {
                        "UPDATE users SET suspended_at=1 WHERE id=?1"
                    } else {
                        "UPDATE users SET deleted_at=1 WHERE id=?1"
                    };
                    sqlx::query(query)
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "revoked" => {
                    sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
                        .bind(f.credential.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "expired" => {
                    sqlx::query("UPDATE sessions SET expires_at=unixepoch()*1000000 WHERE id=?1")
                        .bind(f.credential.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                _ => {}
            }
            let event = f.append_comment_event("comment.created").await;
            push_consumer(None)
                .deliver_backend(&f.backend, owner, &event)
                .await
                .unwrap();
            assert_eq!(counts(&f, event.id).await, (0, 1), "denial {denied}");
            cursor(&f, event.seq).await;
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_subscriptions")
                    .fetch_one(&f.pool)
                    .await
                    .unwrap(),
                1
            );
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn named_queue_checks_system_tenant_writer_cap_and_event_subscription_unique_key() {
        let f = Fixture::new().await;
        prepare(&f, true).await;
        let mut eligible = Vec::new();
        for index in 0..21 {
            let id = subscription(&f, f.user, f.credential, index).await;
            sqlx::query("UPDATE push_subscriptions SET updated_at=?2 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .bind(index as i64)
                .execute(&f.pool)
                .await
                .unwrap();
            if index > 0 {
                eligible.push(id);
            }
        }
        subscription(&f, f.other_user, f.other_credential, 0).await;
        let event = f.append_comment_event("comment.created").await;
        let mut tx = f.backend.begin_write().await.unwrap();
        assert!(tx
            .operation()
            .enqueue_push_deliveries(event.id, f.workspace, &[f.user])
            .await
            .is_err());
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.other_workspace).await.unwrap();
        assert!(tx
            .operation()
            .enqueue_push_deliveries(event.id, f.workspace, &[f.user])
            .await
            .is_err());
        // A family transaction's tenant is immutable. Finish the denied
        // reservation before opening the correctly scoped writer.
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert_eq!(
            tx.operation()
                .enqueue_push_deliveries(event.id, f.workspace, &[f.user])
                .await
                .unwrap(),
            20
        );
        assert_eq!(
            tx.operation()
                .enqueue_push_deliveries(event.id, f.workspace, &[f.user])
                .await
                .unwrap(),
            0
        );
        let crate::db::backend::DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        let rows = family
            .query(
                "SELECT subscription_id FROM push_deliveries ORDER BY subscription_id",
                &[],
            )
            .await
            .unwrap();
        let actual = rows
            .iter()
            .map(|r| r.cell(0).unwrap().id().unwrap())
            .collect::<Vec<_>>();
        eligible.sort_unstable();
        assert_eq!(
            actual, eligible,
            "retain the same newest twenty updated_at/id registrations"
        );
        tx.rollback().await.unwrap();
        assert_eq!(counts(&f, event.id).await, (0, 0));
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .enqueue_push_deliveries(event.id, f.workspace, &[f.user])
            .await
            .is_err());
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_subscriptions")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            22
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn payload_reuses_notification_display_and_existing_korean_text_and_actor_policy() {
        let f = Fixture::new().await;
        prepare(&f, true).await;
        let event = f.append_comment_event("comment.created").await;
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let mut rows = push_recipients_backend(&mut tx.operation(), &event)
            .await
            .unwrap();
        let mut row = rows.remove(&f.user).unwrap();
        let payload = push_payload_backend(&mut tx.operation(), &row)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(payload.title, "한글🙂");
        assert_eq!(payload.body, "문서에 새 댓글이 달렸습니다");
        assert_eq!(payload.url, "/w/notify-main/WIKI-1");
        row.actor_user_id = None;
        assert_eq!(
            push_payload_backend(&mut tx.operation(), &row)
                .await
                .unwrap()
                .unwrap()
                .title,
            "team"
        );
        row.target_type = None;
        row.target_id = None;
        assert_eq!(
            push_payload_backend(&mut tx.operation(), &row)
                .await
                .unwrap()
                .unwrap()
                .url,
            "/w/notify-main/notifications"
        );
        tx.rollback().await.unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(push_payload_backend(&mut tx.operation(), &row)
            .await
            .unwrap()
            .is_none());
        tx.rollback().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn full_schema_rejects_malformed_subscription_uuid_and_null_session() {
        let f = Fixture::new().await;
        for invalid in [false, true] {
            let id = if invalid {
                vec![0; 15]
            } else {
                Uuid::now_v7().as_bytes().to_vec()
            };
            let session = if invalid {
                Some(f.credential.as_bytes().to_vec())
            } else {
                None
            };
            assert!(sqlx::query("INSERT INTO push_subscriptions(id,user_id,session_id,endpoint,p256dh,auth) VALUES(?1,?2,?3,'https://push.example.invalid/malformed',?4,?5)")
                .bind(id).bind(f.user.as_bytes().as_slice()).bind(session).bind(format!("B{}","A".repeat(86))).bind("A".repeat(22))
                .execute(&f.pool).await.is_err());
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM push_subscriptions")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        f.finish().await;
    }
}
