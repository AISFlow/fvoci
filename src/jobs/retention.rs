use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::db::backend::OperationTx;
use crate::db::codec::Cell;
use crate::db::context::set_system;
use crate::integrations::github::GITHUB_CONSUMER;
use crate::integrations::webhooks::WEBHOOKS_CONSUMER;
use crate::mail::MAIL_CONSUMER;
use crate::notifications::NOTIFICATIONS_CONSUMER;
use crate::push::consumer::PUSH_CONSUMER;
use crate::search::index::SEARCH_INDEX_CONSUMER;

pub const NOTIFICATION_READ_RETENTION_DAYS: i32 = 90;
pub const NOTIFICATION_ARCHIVED_RETENTION_DAYS: i32 = 90;
pub const PROCESSED_GC_WINDOW_DAYS: i32 = 30;
pub const GC_DELETE_BATCH: i32 = 5_000;
pub const GC_DELETE_ROUNDS: u32 = 30;

/// Every consumer that writes processed marks (github only while
/// configured). The marks of a consumer missing here are never deleted.
const PROCESSED_GC_CONSUMERS: &[&str] = &[
    NOTIFICATIONS_CONSUMER,
    MAIL_CONSUMER,
    PUSH_CONSUMER,
    SEARCH_INDEX_CONSUMER,
    WEBHOOKS_CONSUMER,
    GITHUB_CONSUMER,
];

/// Source sweep.ts notification + processed_events GC. events, audit_log, and
/// collab receipts are not purged in the source daily sweep (and the app role
/// cannot DELETE them).
pub async fn run_notification_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<(u32, u32), sqlx::Error> {
    let read =
        gc_notification_kind(pool, cancel, "notifications", NotificationGcKind::Read).await?;
    let archived = gc_notification_kind(
        pool,
        cancel,
        "notifications-archived",
        NotificationGcKind::Archived,
    )
    .await?;
    Ok((read, archived))
}

#[derive(Clone, Copy)]
enum NotificationGcKind {
    Read,
    Archived,
}

pub async fn run_processed_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    for consumer in PROCESSED_GC_CONSUMERS {
        if cancel.is_cancelled() {
            break;
        }
        deleted += gc_processed_consumer(pool, consumer, cancel).await?;
    }
    Ok(deleted)
}

async fn gc_processed_consumer(
    pool: &PgPool,
    consumer: &str,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let mut tx = pool.begin().await?;
        set_system(&mut tx).await?;
        let n = OperationTx::Postgres(&mut tx)
            .maintenance_processed_gc(consumer)
            .await?;
        tx.commit().await?;
        deleted += n;
        if n < GC_DELETE_BATCH as u32 {
            break;
        }
    }
    Ok(deleted)
}

async fn gc_notification_kind(
    pool: &PgPool,
    cancel: &CancellationToken,
    kind: &'static str,
    which: NotificationGcKind,
) -> Result<u32, sqlx::Error> {
    let mut deleted = 0u32;
    let mut rounds = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = match which {
            NotificationGcKind::Read => purge_read_batch(pool).await?,
            NotificationGcKind::Archived => purge_archived_batch(pool).await?,
        };
        rounds += 1;
        deleted += n;
        if n < GC_DELETE_BATCH as u32 {
            return Ok(deleted);
        }
    }
    tracing::warn!(kind, deleted, rounds, "cleanup.gc_round_capped");
    Ok(deleted)
}

async fn purge_read_batch(pool: &PgPool) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = OperationTx::Postgres(&mut tx)
        .maintenance_notification_gc(NotificationGcKind::Read)
        .await?;
    tx.commit().await?;
    Ok(deleted)
}

async fn purge_archived_batch(pool: &PgPool) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = OperationTx::Postgres(&mut tx)
        .maintenance_notification_gc(NotificationGcKind::Archived)
        .await?;
    tx.commit().await?;
    Ok(deleted)
}

impl OperationTx<'_, '_> {
    async fn maintenance_notification_gc(
        &mut self,
        which: NotificationGcKind,
    ) -> Result<u32, sqlx::Error> {
        let deleted = match self {
            Self::Postgres(tx) => match which {
                NotificationGcKind::Read => sqlx::query(
                    r#"
        WITH doomed AS (
            SELECT ctid
            FROM fvoci.notifications
            WHERE read_at IS NOT NULL
              AND read_at < now() - ($1::text || ' days')::interval
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.notifications AS n
        USING doomed
        WHERE n.ctid = doomed.ctid
        "#,
                )
                .bind(NOTIFICATION_READ_RETENTION_DAYS.to_string())
                .bind(GC_DELETE_BATCH)
                .execute(&mut ***tx)
                .await?
                .rows_affected(),
                NotificationGcKind::Archived => sqlx::query(
                    r#"
        WITH doomed AS (
            SELECT ctid
            FROM fvoci.notifications
            WHERE archived_at IS NOT NULL
              AND read_at IS NULL
              AND archived_at < now() - ($1::text || ' days')::interval
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.notifications AS n
        USING doomed
        WHERE n.ctid = doomed.ctid
        "#,
                )
                .bind(NOTIFICATION_ARCHIVED_RETENTION_DAYS.to_string())
                .bind(GC_DELETE_BATCH)
                .execute(&mut ***tx)
                .await?
                .rows_affected(),
            },
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let (sql, days) = match which {
                    NotificationGcKind::Read => (
                        "DELETE FROM notifications WHERE id IN (SELECT id FROM notifications WHERE read_at IS NOT NULL AND read_at < (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?1 LIMIT ?2)",
                        NOTIFICATION_READ_RETENTION_DAYS,
                    ),
                    NotificationGcKind::Archived => (
                        "DELETE FROM notifications WHERE id IN (SELECT id FROM notifications WHERE archived_at IS NOT NULL AND read_at IS NULL AND archived_at < (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?1 LIMIT ?2)",
                        NOTIFICATION_ARCHIVED_RETENTION_DAYS,
                    ),
                };
                tx.execute(
                    sql,
                    &[
                        Cell::Integer(i64::from(days) * 86_400_000_000),
                        Cell::Integer(i64::from(GC_DELETE_BATCH)),
                    ],
                )
                .await?
            }
        };
        u32::try_from(deleted)
            .map_err(|_| sqlx::Error::Protocol("notification cleanup count overflow".into()))
    }

    pub(crate) async fn maintenance_processed_gc(
        &mut self,
        consumer: &str,
    ) -> Result<u32, sqlx::Error> {
        // The maintenance registry is the complete current set of consumers.
        // An arbitrary name must not widen the global deletion operation.
        if !PROCESSED_GC_CONSUMERS.contains(&consumer) {
            return Err(sqlx::Error::Protocol(
                "unknown maintenance processed consumer".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                let n: i32 = sqlx::query_scalar("SELECT fvoci.app_outbox_gc_processed($1, $2, $3)")
                    .bind(consumer)
                    .bind(PROCESSED_GC_WINDOW_DAYS)
                    .bind(GC_DELETE_BATCH)
                    .fetch_one(&mut ***tx)
                    .await?;
                Ok(n.max(0) as u32)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                // Family events have a serialized seq instead of PostgreSQL
                // xid8 visibility. Keep the highest processed position and
                // retain missing-event marks, as the original inner join does.
                let n = tx.execute(
                    "WITH latest AS (SELECT max(e.seq) AS seq FROM processed_events p INNER JOIN events e ON e.id=p.event_id WHERE p.consumer=?1), doomed AS (SELECT p.event_id FROM processed_events p INNER JOIN events e ON e.id=p.event_id WHERE p.consumer=?1 AND e.seq < (SELECT seq FROM latest) AND p.processed_at < (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-?2 LIMIT ?3) DELETE FROM processed_events WHERE consumer=?1 AND event_id IN (SELECT event_id FROM doomed)",
                    &[Cell::text(consumer), Cell::Integer(i64::from(PROCESSED_GC_WINDOW_DAYS)*86_400_000_000), Cell::Integer(i64::from(GC_DELETE_BATCH))],
                ).await?;
                u32::try_from(n)
                    .map_err(|_| sqlx::Error::Protocol("processed cleanup count overflow".into()))
            }
        }
    }
}

/// Source `gcWebhookDeliveries` (90 days), the GitHub delivery-id dedupe
/// rows (30 days) and expired install states, in bounded batches.
pub async fn run_integration_gc(
    pool: &PgPool,
    cancel: &CancellationToken,
) -> Result<(u32, u32), sqlx::Error> {
    use crate::db::integrations::{
        purge_expired_install_states, purge_github_deliveries, purge_settled_deliveries,
        GITHUB_DELIVERY_RETENTION_DAYS, INTEGRATION_GC_BATCH, WEBHOOK_DELIVERY_RETENTION_DAYS,
    };
    let mut webhook = 0u32;
    let mut github = 0u32;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = purge_settled_deliveries(pool, WEBHOOK_DELIVERY_RETENTION_DAYS).await?;
        webhook += n as u32;
        if (n as i64) < INTEGRATION_GC_BATCH {
            break;
        }
    }
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = purge_github_deliveries(pool, GITHUB_DELIVERY_RETENTION_DAYS).await?;
        github += n as u32;
        if (n as i64) < INTEGRATION_GC_BATCH {
            break;
        }
    }
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            break;
        }
        let n = purge_expired_install_states(pool).await?;
        github += n as u32;
        if (n as i64) < INTEGRATION_GC_BATCH {
            break;
        }
    }
    Ok((webhook, github))
}

#[derive(Clone, Copy)]
enum RetentionBatch {
    Notification(NotificationGcKind),
    Processed(&'static str),
    Webhook,
    Github,
    InstallState,
}

pub(crate) async fn run_notification_gc_family(
    backend: &crate::db::backend::Backend,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<(u32, u32), super::MaintenanceConsumerError> {
    let read = run_retention_batches_family(
        backend,
        proof,
        policy,
        cancel,
        RetentionBatch::Notification(NotificationGcKind::Read),
    )
    .await?;
    let archived = run_retention_batches_family(
        backend,
        proof,
        policy,
        cancel,
        RetentionBatch::Notification(NotificationGcKind::Archived),
    )
    .await?;
    Ok((read, archived))
}

pub(crate) async fn run_processed_gc_family(
    backend: &crate::db::backend::Backend,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<u32, super::MaintenanceConsumerError> {
    let mut deleted = 0;
    for consumer in PROCESSED_GC_CONSUMERS {
        deleted += run_retention_batches_family(
            backend,
            proof,
            policy,
            cancel,
            RetentionBatch::Processed(consumer),
        )
        .await?;
    }
    Ok(deleted)
}

pub(crate) async fn run_integration_gc_family(
    backend: &crate::db::backend::Backend,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<(u32, u32), super::MaintenanceConsumerError> {
    let webhook =
        run_retention_batches_family(backend, proof, policy, cancel, RetentionBatch::Webhook)
            .await?;
    let github =
        run_retention_batches_family(backend, proof, policy, cancel, RetentionBatch::Github)
            .await?
            + run_retention_batches_family(
                backend,
                proof,
                policy,
                cancel,
                RetentionBatch::InstallState,
            )
            .await?;
    Ok((webhook, github))
}

async fn run_retention_batches_family(
    backend: &crate::db::backend::Backend,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
    which: RetentionBatch,
) -> Result<u32, super::MaintenanceConsumerError> {
    use super::{commit_maintenance_writer, renew_maintenance_writer, rollback_maintenance_writer};
    use crate::db::integrations::{
        GITHUB_DELIVERY_RETENTION_DAYS, INTEGRATION_GC_BATCH, WEBHOOK_DELIVERY_RETENTION_DAYS,
    };
    let key = super::claim::MaintenanceJobKey::Daily;
    let mut deleted = 0;
    for _ in 0..GC_DELETE_ROUNDS {
        if cancel.is_cancelled() {
            return Err(super::MaintenanceConsumerError::Cancelled);
        }
        let mut tx = backend.begin_write().await?;
        let result = async {
            renew_maintenance_writer(&mut tx, proof, key, policy, cancel).await?;
            let previous = tx.operation().set_system().await?;
            let n = match which {
                RetentionBatch::Notification(kind) => {
                    u64::from(tx.operation().maintenance_notification_gc(kind).await?)
                }
                RetentionBatch::Processed(consumer) => {
                    u64::from(tx.operation().maintenance_processed_gc(consumer).await?)
                }
                RetentionBatch::Webhook => {
                    tx.operation()
                        .webhook_purge_settled(WEBHOOK_DELIVERY_RETENTION_DAYS)
                        .await?
                }
                RetentionBatch::Github => {
                    tx.operation()
                        .maintenance_github_delivery_gc(GITHUB_DELIVERY_RETENTION_DAYS)
                        .await?
                }
                RetentionBatch::InstallState => {
                    tx.operation().maintenance_install_state_gc().await?
                }
            };
            tx.operation().restore_system(previous).await?;
            let n = u32::try_from(n).map_err(|_| {
                sqlx::Error::Protocol("maintenance retention count overflow".into())
            })?;
            Ok::<_, super::MaintenanceConsumerError>(n)
        }
        .await;
        let n = match result {
            Ok(n) => n,
            Err(err) => return Err(rollback_maintenance_writer(tx, err).await),
        };
        commit_maintenance_writer(tx, proof, key, cancel).await?;
        deleted += n;
        let batch = match which {
            RetentionBatch::Webhook | RetentionBatch::Github | RetentionBatch::InstallState => {
                INTEGRATION_GC_BATCH as u32
            }
            _ => GC_DELETE_BATCH as u32,
        };
        if n < batch {
            return Ok(deleted);
        }
    }
    if let RetentionBatch::Notification(kind) = which {
        let kind = match kind {
            NotificationGcKind::Read => "notifications",
            NotificationGcKind::Archived => "notifications-archived",
        };
        tracing::warn!(
            kind,
            deleted,
            rounds = GC_DELETE_ROUNDS,
            "cleanup.gc_round_capped"
        );
    }
    Ok(deleted)
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::db::identity::EventAppend;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use serde_json::json;
    use uuid::Uuid;

    #[tokio::test]
    async fn maintenance_retention_exact_predicates_and_current_processed_position() {
        let f = Fixture::new().await;
        let read_old = Uuid::now_v7();
        let archived_old = Uuid::now_v7();
        let archived_but_read_recent = Uuid::now_v7();
        let unread = Uuid::now_v7();
        for (id, read, archived) in [
            (read_old, Some(1), None),
            (archived_old, None, Some(1)),
            (archived_but_read_recent, Some(i64::MAX), Some(1)),
            (unread, None, None),
        ] {
            sqlx::query("INSERT INTO notifications(id,workspace_id,user_id,event_id,verb,read_at,archived_at) VALUES(?1,?2,?3,?4,'fixture',?5,?6)")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice())
                .bind(read).bind(archived).execute(&f.pool).await.unwrap();
        }
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let highest = Uuid::now_v7();
        let missing = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        assert!(tx
            .operation()
            .maintenance_notification_gc(NotificationGcKind::Read)
            .await
            .is_err());
        let previous = tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        for event in [first, second, highest] {
            tx.operation()
                .append_event(EventAppend {
                    id: event,
                    workspace_id: Some(f.workspace),
                    actor_user_id: None,
                    verb: "fixture".into(),
                    target_type: None,
                    target_id: None,
                    payload: json!({}),
                })
                .await
                .unwrap();
        }
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual family writer required")
        };
        for event in [first, second, highest, missing] {
            writer
                .execute(
                    "INSERT INTO processed_events(consumer,event_id,processed_at) VALUES(?1,?2,1)",
                    &[Cell::text(NOTIFICATIONS_CONSUMER), Cell::uuid(event)],
                )
                .await
                .unwrap();
        }
        writer.execute("INSERT INTO processed_events(consumer,event_id,processed_at) VALUES('unregistered',?1,1)", &[Cell::uuid(first)]).await.unwrap();
        assert!(tx
            .operation()
            .maintenance_processed_gc("unregistered")
            .await
            .is_err());
        assert_eq!(
            tx.operation()
                .maintenance_notification_gc(NotificationGcKind::Read)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            tx.operation()
                .maintenance_notification_gc(NotificationGcKind::Archived)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            tx.operation()
                .maintenance_processed_gc(NOTIFICATIONS_CONSUMER)
                .await
                .unwrap(),
            2
        );
        tx.operation().restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        let remaining: Vec<Vec<u8>> =
            sqlx::query_scalar("SELECT id FROM notifications ORDER BY id")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        let mut expected = vec![
            archived_but_read_recent.as_bytes().to_vec(),
            unread.as_bytes().to_vec(),
        ];
        expected.sort();
        assert_eq!(remaining, expected);
        let marks: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT event_id FROM processed_events WHERE consumer=?1 ORDER BY event_id",
        )
        .bind(NOTIFICATIONS_CONSUMER)
        .fetch_all(&f.pool)
        .await
        .unwrap();
        let mut expected = vec![highest.as_bytes().to_vec(), missing.as_bytes().to_vec()];
        expected.sort();
        assert_eq!(marks, expected);
        let original_events: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(original_events, 3);
        let unrelated: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM processed_events WHERE consumer='unregistered'",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(unrelated, 1);
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
}
