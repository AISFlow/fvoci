//! Named daily digest operations. Counts retain unread notification metadata;
//! no protected target content, IDs, links or payload are loaded for the mail.
use crate::db::backend::OperationTx;
use crate::db::codec::Cell;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub(crate) type DigestClaim = (Uuid, Uuid, Option<DateTime<Utc>>);

impl OperationTx<'_, '_> {
    pub(crate) async fn digest_claim_due(
        &mut self,
        before: DateTime<Utc>,
        now: DateTime<Utc>,
        after: Option<(Uuid, Uuid)>,
        limit: i64,
    ) -> Result<Vec<DigestClaim>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_as(
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
                .bind(limit)
                .bind(after.map(|(w, _)| w))
                .bind(after.map(|(_, u)| u))
                .fetch_all(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_writer()?;
                let rows=tx.query("SELECT workspace_id,user_id,last_digest_at FROM notification_prefs WHERE mail_digest=1 AND (last_digest_at IS NULL OR last_digest_at<=?1) AND (?2 IS NULL OR (workspace_id,user_id)>(?2,?3)) ORDER BY workspace_id,user_id LIMIT ?4",&[Cell::instant(before)?,Cell::optional_uuid(after.map(|(w,_)|w)),Cell::optional_uuid(after.map(|(_,u)|u)),Cell::Integer(limit)]).await?;
                let mut claims = Vec::new();
                for row in rows {
                    let workspace = row.cell(0)?.id()?;
                    let user = row.cell(1)?.id()?;
                    let previous = row.cell(2)?.optional(Cell::datetime)?;
                    let changed=tx.execute("UPDATE notification_prefs SET last_digest_at=?3,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND user_id=?2 AND mail_digest=1 AND (last_digest_at IS NULL OR last_digest_at<=?4)",&[Cell::uuid(workspace),Cell::uuid(user),Cell::instant(now)?,Cell::instant(before)?]).await?;
                    if changed != 1 {
                        return Err(sqlx::Error::Protocol(
                            "digest claim changed under writer reservation".into(),
                        ));
                    }
                    claims.push((workspace, user, previous));
                }
                Ok(claims)
            }
        }
    }
    pub(crate) async fn digest_restore_claim(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        previous: Option<DateTime<Utc>>,
        claimed_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query(
                r#"
        UPDATE fvoci.notification_prefs
        SET last_digest_at = $3, updated_at = now()
        WHERE workspace_id = $1 AND user_id = $2 AND last_digest_at = $4
        "#,
            )
            .bind(workspace)
            .bind(user)
            .bind(previous)
            .bind(claimed_at)
            .execute(&mut ***tx)
            .await?
            .rows_affected()
                == 1),
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let previous = previous
                    .map(Cell::instant)
                    .transpose()?
                    .unwrap_or(Cell::Null);
                let changed=tx.execute("UPDATE notification_prefs SET last_digest_at=?3,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND user_id=?2 AND last_digest_at=?4",&[Cell::uuid(workspace),Cell::uuid(user),previous,Cell::instant(claimed_at)?]).await?;
                Ok(changed == 1)
            }
        }
    }
    pub(crate) async fn digest_confirm_claim(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        claimed_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query_scalar::<_,bool>("SELECT true FROM fvoci.notification_prefs WHERE workspace_id=$1 AND user_id=$2 AND mail_digest=true AND last_digest_at=$3").bind(workspace).bind(user).bind(claimed_at).fetch_optional(&mut ***tx).await?.unwrap_or(false)),
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT mail_digest FROM notification_prefs WHERE workspace_id=?1 AND user_id=?2 AND last_digest_at=?3",&[Cell::uuid(workspace),Cell::uuid(user),Cell::instant(claimed_at)?]).await?;
                Ok(rows.first().map(|r|r.cell(0)?.boolean()).transpose()?.unwrap_or(false))
            }
        }
    }
    /// Resolve the recipient from current authority in this read transaction.
    pub(crate) async fn digest_recipient(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        claimed_at: DateTime<Utc>,
    ) -> Result<Option<String>, sqlx::Error> {
        if !self.workspace_is_live(workspace).await?
            || self
                .membership_role(workspace, user, false)
                .await?
                .is_none()
            || !self
                .digest_confirm_claim(workspace, user, claimed_at)
                .await?
        {
            return Ok(None);
        }
        Ok(crate::db::notifications::mail_user(self, user)
            .await?
            .map(|(email, _, _)| email))
    }
    pub(crate) async fn digest_unread_count(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        previous: Option<DateTime<Utc>>,
    ) -> Result<i64, sqlx::Error> {
        let count = match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar(
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
                .bind(workspace)
                .bind(user)
                .bind(previous)
                .fetch_one(&mut ***tx)
                .await?
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.require_tenant(workspace)?;
                let previous = previous
                    .map(Cell::instant)
                    .transpose()?
                    .unwrap_or(Cell::Null);
                let rows=tx.query("SELECT count(*) FROM notifications WHERE workspace_id=?1 AND user_id=?2 AND read_at IS NULL AND archived_at IS NULL AND (?3 IS NULL OR created_at>=?3)",&[Cell::uuid(workspace),Cell::uuid(user),previous]).await?;
                rows.first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .integer()?
            }
        };
        if count < 0 {
            return Err(sqlx::Error::Protocol("negative digest count".into()));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod family_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;

    async fn prefs(f: &Fixture, w: Uuid, u: Uuid) {
        sqlx::query(
            "INSERT INTO notification_prefs(workspace_id,user_id,mail_digest) VALUES(?1,?2,1)",
        )
        .bind(w.as_bytes().as_slice())
        .bind(u.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    }
    async fn claim(
        backend: &crate::db::backend::Backend,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Vec<DigestClaim> {
        let mut tx = backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        let rows = tx
            .operation()
            .digest_claim_due(now - chrono::Duration::days(1), now, None, limit)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        rows
    }
    #[tokio::test]
    async fn actual_family_claims_serialize_across_connections_and_walk_keys() {
        let f = Fixture::new().await;
        prefs(&f, f.workspace, f.user).await;
        prefs(&f, f.other_workspace, f.other_user).await;
        let other = crate::db::backend::Backend::Sqlite(
            crate::db::pool::connect_sqlite_app(&f.dir.join("test.sqlite"), 1)
                .await
                .unwrap(),
        );
        let now = DateTime::from_timestamp_micros(1_790_000_000_000_123).unwrap();
        let (a, b) = tokio::join!(claim(&f.backend, now, 100), claim(&other, now, 100));
        assert_eq!(
            a.len() + b.len(),
            2,
            "each due row is claimed once across actual connections"
        );
        assert!(a.is_empty() || b.is_empty());
        assert!(claim(&f.backend, now, 100).await.is_empty());
        sqlx::query("UPDATE notification_prefs SET last_digest_at=NULL")
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        let first = tx
            .operation()
            .digest_claim_due(now - chrono::Duration::days(1), now, None, 1)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let key = (first[0].0, first[0].1);
        let mut expected = vec![(f.workspace, f.user), (f.other_workspace, f.other_user)];
        expected.sort();
        assert_eq!(key, expected[0]);
        let last = tx
            .operation()
            .digest_claim_due(now - chrono::Duration::days(1), now, Some(key), 1)
            .await
            .unwrap();
        assert_eq!((last[0].0, last[0].1), expected[1]);
        tx.commit().await.unwrap();
        other.close().await.unwrap();
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_family_restore_is_fenced_and_preserves_null_and_microseconds() {
        let f = Fixture::new().await;
        prefs(&f, f.workspace, f.user).await;
        let now = DateTime::from_timestamp_micros(1_790_000_000_000_123).unwrap();
        let claims = claim(&f.backend, now, 100).await;
        assert_eq!(claims, vec![(f.workspace, f.user, None)]);
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.other_workspace).await.unwrap();
        assert!(tx
            .operation()
            .digest_restore_claim(f.workspace, f.user, None, now)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let newer = now + chrono::Duration::microseconds(1);
        sqlx::query("UPDATE notification_prefs SET last_digest_at=?1")
            .bind(newer.timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(!tx
            .operation()
            .digest_restore_claim(f.workspace, f.user, None, now)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .digest_confirm_claim(f.workspace, f.user, newer)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .digest_restore_claim(f.workspace, f.user, Some(now), newer)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .digest_confirm_claim(f.workspace, f.user, now)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .digest_restore_claim(f.workspace, f.user, None, now)
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let at: Option<i64> = sqlx::query_scalar("SELECT last_digest_at FROM notification_prefs")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(at, None);
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_family_count_preserves_owner_window_and_retained_target_metadata() {
        let f = Fixture::new().await;
        for (id, at) in [(1u128, 99i64), (2, 100), (3, 101), (4, 102), (5, 103)] {
            f.insert_inbox(Uuid::from_u128(id), at).await;
        }
        sqlx::query("UPDATE notifications SET read_at=104 WHERE id=?1")
            .bind(Uuid::from_u128(4).as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE notifications SET archived_at=104 WHERE id=?1")
            .bind(Uuid::from_u128(5).as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE documents SET deleted_at=105,title='protected-title-never-loaded' WHERE id=?1",
        )
        .bind(f.document.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert_eq!(
            tx.operation()
                .digest_unread_count(f.workspace, f.user, None)
                .await
                .unwrap(),
            3
        );
        assert_eq!(tx.operation().digest_unread_count(f.workspace,f.user,Some(DateTime::from_timestamp_micros(100).unwrap())).await.unwrap(),2,"created_at==prevLast remains in the window; deleted target retains count-only metadata");
        assert_eq!(
            tx.operation()
                .digest_unread_count(f.workspace, f.actor, None)
                .await
                .unwrap(),
            0
        );
        assert!(tx
            .operation()
            .digest_unread_count(f.other_workspace, f.other_user, None)
            .await
            .is_err());
        tx.commit().await.unwrap();
        f.finish().await;
    }
    #[tokio::test]
    async fn actual_family_cancelled_claim_rolls_back_before_connection_reuse() {
        let f = Fixture::new().await;
        prefs(&f, f.workspace, f.user).await;
        let now = DateTime::from_timestamp_micros(1_790_000_000_000_123).unwrap();
        let backend = f.backend.clone();
        let (ready, rx) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation().set_system().await.unwrap();
            let claimed = tx
                .operation()
                .digest_claim_due(now - chrono::Duration::days(1), now, None, 100)
                .await
                .unwrap();
            assert_eq!(claimed.len(), 1);
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            tx.commit().await.unwrap();
        });
        rx.await.unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        assert_eq!(
            claim(&f.backend, now, 100).await,
            vec![(f.workspace, f.user, None)],
            "cancelled uncommitted claim must roll back before the same max1 connection is reused"
        );
        f.finish().await;
    }
}
