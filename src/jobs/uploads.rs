//! Abandoned / expired multipart upload cleanup.
//!
//! Ports `gcStaleUploadRow` in `packages/jobs/src/sweep.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843`. The source runs it inside the
//! daily sweep; here it is its own scheduler job with a shorter cadence,
//! because the TTL is hours long and stale S3 multipart uploads keep costing
//! storage until aborted. Concurrent complete holds the same upload session
//! lock across publish on PostgreSQL. Selected-family assembling rows remain
//! busy until S18 supplies the actual shared writer/session authority owner;
//! the finite cleanup consumer is not that missing complete integration.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::attachments::ObjectStorage;
use crate::db::attachments::{
    gc_stale_upload_row_backend_with_cancel, list_stale_uploading_backend, StaleUploadCursor,
};
use crate::db::backend::Backend;

/// Rows examined per run. Anything left over waits for the next cadence.
pub const UPLOAD_GC_BATCH: i64 = 200;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaleUploadGcStats {
    pub claimed: u32,
    pub purged: u32,
    /// Rows whose storage cleanup failed; kept for the next run.
    pub failed: u32,
    /// Where the next run resumes; `None` once the listing wrapped around.
    pub resume_after: Option<StaleUploadCursor>,
}

/// Removes up to `limit` incomplete uploads created before `cutoff`, resuming
/// after `after`: every open multipart upload for the key is aborted, the
/// object deleted, then the row. A row whose storage cleanup fails keeps its
/// DB row and does not stop the rest.
pub async fn run_stale_upload_gc(
    pool: &PgPool,
    storage: &ObjectStorage,
    cutoff: DateTime<Utc>,
    after: Option<StaleUploadCursor>,
    limit: i64,
    cancel: &CancellationToken,
) -> Result<StaleUploadGcStats, sqlx::Error> {
    run_stale_upload_gc_backend(
        &Backend::Postgres(pool.clone()),
        storage,
        cutoff,
        after,
        limit,
        cancel,
    )
    .await
}

/// Finite selected-backend batch; scheduler/global job claim remains owned by
/// the separate S16 consumer. Family busy assembling rows still advance the
/// actual global microsecond cursor so later healthy tenants are not starved.
pub async fn run_stale_upload_gc_backend(
    backend: &Backend,
    storage: &ObjectStorage,
    cutoff: DateTime<Utc>,
    after: Option<StaleUploadCursor>,
    limit: i64,
    cancel: &CancellationToken,
) -> Result<StaleUploadGcStats, sqlx::Error> {
    let rows = list_stale_uploading_backend(backend, cutoff, after, limit).await?;
    let full_batch = rows.len() as i64 >= limit;
    let mut stats = StaleUploadGcStats::default();
    let mut last = None;
    for row in rows {
        if cancel.is_cancelled() {
            // Resume at the first row not examined.
            stats.resume_after = last.or(after);
            return Ok(stats);
        }
        last = Some((row.created_at, row.id));
        stats.claimed += 1;
        match gc_stale_upload_row_backend_with_cancel(
            backend,
            storage,
            row.workspace_id,
            row.id,
            cancel,
        )
        .await
        {
            Ok(true) => stats.purged += 1,
            Ok(false) => {}
            Err(err) => {
                stats.failed += 1;
                warn!(attachment_id = %row.id, error = %err, "maintenance.upload_gc_row_failed");
            }
        }
    }
    // A short batch reached the end of the order: start over next time.
    stats.resume_after = if full_batch { last } else { None };
    Ok(stats)
}

/// Scheduler's claimed family batch. Cutoff/cursor/limit remain caller policy;
/// this is not a hardcoded TTL or a global authority preflight. Every row uses
/// the same real writer for its Uploads proof, current row/storage and COMMIT.
/// PG callers retain run_stale_upload_gc and the detached JobClaim wrapper.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_stale_upload_gc_claimed_backend(
    backend: &Backend,
    storage: &ObjectStorage,
    cutoff: DateTime<Utc>,
    after: Option<StaleUploadCursor>,
    limit: i64,
    cancel: &CancellationToken,
    proof: &crate::db::maintenance_claim::FamilyMaintenanceProof,
    policy: crate::db::maintenance_claim::FamilyMaintenanceLeasePolicy,
) -> Result<StaleUploadGcStats, sqlx::Error> {
    use crate::db::attachments::{
        gc_stale_upload_row_claimed_backend, upload_maintenance_must_stop, UploadMaintenanceStop,
    };
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "family Uploads proof cannot replace PostgreSQL detached job ownership".into(),
        ));
    }
    if cancel.is_cancelled() {
        return Err(sqlx::Error::AnyDriverError(Box::new(
            UploadMaintenanceStop::Cancelled,
        )));
    }
    let rows = list_stale_uploading_backend(backend, cutoff, after, limit).await?;
    let full_batch = rows.len() as i64 >= limit;
    let mut stats = StaleUploadGcStats::default();
    let mut last = None;
    for row in rows {
        if cancel.is_cancelled() {
            return Err(sqlx::Error::AnyDriverError(Box::new(
                UploadMaintenanceStop::Cancelled,
            )));
        }
        match gc_stale_upload_row_claimed_backend(
            backend,
            storage,
            row.workspace_id,
            row.id,
            proof,
            policy,
            cancel,
        )
        .await
        {
            Ok(removed) => {
                stats.claimed += 1;
                if removed {
                    stats.purged += 1;
                }
            }
            Err(error)
                if matches!(backend, Backend::LibsqlRemote(_))
                    || upload_maintenance_must_stop(&error) =>
            {
                return Err(error)
            }
            Err(error) => {
                stats.claimed += 1;
                stats.failed += 1;
                warn!(attachment_id=%row.id,error=%error,"maintenance.claimed_upload_gc_row_failed");
            }
        }
        // Advance over each genuinely examined busy/failed/healthy tuple;
        // uncertainty/loss returns above and grants no new resume receipt.
        last = Some((row.created_at, row.id));
    }
    stats.resume_after = if full_batch { last } else { None };
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::attachments::cleanup_tests::{storage, Fixture};
    use crate::db::attachments::{cleanup_test_hooks, reclaim_attachment_objects_backend};
    use chrono::TimeZone;
    use uuid::Uuid;

    async fn upload(f: &Fixture, status: &str, at: i64) -> (Uuid, String) {
        let (id, key) = f.attachment(4, "application/octet-stream").await;
        sqlx::query("UPDATE attachments SET status=?2,size_bytes=NULL,completed_at=NULL,created_at=?3 WHERE id=?1").bind(id.as_bytes().as_slice()).bind(status).bind(at).execute(&f.pool).await.unwrap();
        (id, key)
    }
    async fn exists(f: &Fixture, id: Uuid) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments WHERE id=?1")
            .bind(id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap()
            == 1
    }

    #[tokio::test]
    async fn cleanup_selected_stale_global_microsecond_pages_busy_failed_healthy() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let base = 1_700_000_000_000_000i64;
        let cutoff = DateTime::from_timestamp_micros(base + 10).unwrap();
        let (busy, busykey) = upload(&f, "assembling", base + 1).await;
        let (failed, failedkey) = upload(&f, "uploading", base + 2).await;
        let (healthy, healthykey) = upload(&f, "uploading", base + 3).await;
        let (at_cutoff, _) = upload(&f, "uploading", base + 10).await;
        let other = Uuid::now_v7();
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'s17-other','S17 other')")
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'S17',?3,'V',1,'published',2,?4,'{}')").bind(document.as_bytes().as_slice()).bind(other.as_bytes().as_slice()).bind(document.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE attachments SET workspace_id=?2,document_id=?3 WHERE id=?1")
            .bind(healthy.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .bind(document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for key in [&busykey, &failedkey, &healthykey] {
            s.put_bytes(key, b"part".to_vec()).await.unwrap();
        }
        let blocked = f.root.join("s17-storage/tmp").join(&failedkey);
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        std::fs::write(&blocked, b"real abort ENOTDIR").unwrap();
        let rows = list_stale_uploading_backend(&f.backend, cutoff, None, 20)
            .await
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| (r.id, r.created_at.timestamp_micros()))
                .collect::<Vec<_>>(),
            vec![(busy, base + 1), (failed, base + 2), (healthy, base + 3)]
        );
        assert_eq!(rows[2].workspace_id, other);
        assert!(
            list_stale_uploading_backend(
                &f.backend,
                Utc.timestamp_opt(1_700_000_000, 1).unwrap(),
                None,
                20
            )
            .await
            .is_err(),
            "cutoff must not silently lose nanoseconds"
        );
        let cancel = CancellationToken::new();
        let first = run_stale_upload_gc_backend(&f.backend, &s, cutoff, None, 1, &cancel)
            .await
            .unwrap();
        assert_eq!((first.claimed, first.purged, first.failed), (1, 0, 0));
        assert_eq!(
            first.resume_after.unwrap(),
            (DateTime::from_timestamp_micros(base + 1).unwrap(), busy)
        );
        let second =
            run_stale_upload_gc_backend(&f.backend, &s, cutoff, first.resume_after, 1, &cancel)
                .await
                .unwrap();
        assert_eq!((second.claimed, second.purged, second.failed), (1, 0, 1));
        assert_eq!(second.resume_after.unwrap().1, failed);
        let third =
            run_stale_upload_gc_backend(&f.backend, &s, cutoff, second.resume_after, 1, &cancel)
                .await
                .unwrap();
        assert_eq!((third.claimed, third.purged, third.failed), (1, 1, 0));
        assert!(!exists(&f, healthy).await);
        assert_eq!(s.head(&healthykey).await.unwrap(), None);
        let end =
            run_stale_upload_gc_backend(&f.backend, &s, cutoff, third.resume_after, 1, &cancel)
                .await
                .unwrap();
        assert_eq!((end.claimed, end.resume_after), (0, None));
        let wrapped =
            run_stale_upload_gc_backend(&f.backend, &s, cutoff, end.resume_after, 1, &cancel)
                .await
                .unwrap();
        assert_eq!(wrapped.resume_after.unwrap().1, busy);
        assert!(exists(&f, busy).await && exists(&f, failed).await && exists(&f, at_cutoff).await);
        assert_eq!(s.read_range(&busykey, 0, 3).await.unwrap(), b"part");
        std::fs::remove_file(blocked).unwrap();
        let positive =
            run_stale_upload_gc_backend(&f.backend, &s, cutoff, first.resume_after, 20, &cancel)
                .await
                .unwrap();
        assert_eq!(
            (positive.claimed, positive.purged, positive.failed),
            (1, 1, 0)
        );
        let journal: Vec<String> = sqlx::query_scalar(
            "SELECT storage_key FROM attachment_object_cleanups ORDER BY storage_key",
        )
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert_eq!(journal.len(), 2);
        assert!(journal.contains(&healthykey) && journal.contains(&failedkey));
        assert_eq!(
            reclaim_attachment_objects_backend(&f.backend, &s, None, 20)
                .await
                .unwrap()
                .reclaimed,
            2
        );
        println!("S17 actual global stale pages preserve microseconds, busy/failure continuation, other tenant healthy progress and trigger journal");
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_stale_cancellation_keeps_row_then_explicit_retry() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (id, key) = upload(&f, "uploading", 1).await;
        s.put_bytes(&key, b"part".to_vec()).await.unwrap();
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            run_stale_upload_gc_backend(
                &f.backend,
                &s,
                DateTime::from_timestamp_micros(10).unwrap(),
                None,
                1,
                &token
            )
            .await
            .unwrap()
            .claimed,
            0
        );
        assert!(exists(&f, id).await);
        assert_eq!(s.head(&key).await.unwrap(), Some(4));
        let (wait, go) = cleanup_test_hooks::arm(id, 1);
        let backend = f.backend.clone();
        let st = s.clone();
        let token = CancellationToken::new();
        let c = token.clone();
        let run = tokio::spawn(async move {
            run_stale_upload_gc_backend(
                &backend,
                &st,
                DateTime::from_timestamp_micros(10).unwrap(),
                None,
                1,
                &c,
            )
            .await
            .unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), wait)
            .await
            .unwrap()
            .unwrap();
        token.cancel();
        go.send(()).unwrap();
        let result = run.await.unwrap();
        assert_eq!((result.claimed, result.purged, result.failed), (1, 0, 0));
        assert!(exists(&f, id).await);
        assert_eq!(s.head(&key).await.unwrap(), None);
        let healthy = run_stale_upload_gc_backend(
            &f.backend,
            &s,
            DateTime::from_timestamp_micros(10).unwrap(),
            None,
            1,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(healthy.purged, 1);
        assert!(!exists(&f, id).await);
        assert_eq!(f.journals().await.len(), 1);
        assert_eq!(
            reclaim_attachment_objects_backend(&f.backend, &s, None, 20)
                .await
                .unwrap()
                .reclaimed,
            1
        );
        f.close().await;
    }
}
