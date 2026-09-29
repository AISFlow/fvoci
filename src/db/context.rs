use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const MEMBERSHIP_LOCK_NAMESPACE: i32 = 1_907_006;
pub const TREE_LOCK_NAMESPACE: i32 = 1_907_005;
/// Per-workspace search index lock (transaction-scoped). Not 1_907_007: that is
/// the collab room fence, a session lock held for a room's lifetime, which
/// stalled a workspace's indexing whenever a document id and the workspace id
/// shared their low 32 bits.
pub const SEARCH_INDEX_LOCK_NAMESPACE: i32 = 1_907_009;
pub const SEARCH_REBUILD_LOCK_KEY: i64 = 1_907_008;

tokio::task_local! {
    /// Import job whose run is in progress on this task. Every tenant
    /// transaction begun inside [`defer_import_events`] parks its events for
    /// that job (migration 032) instead of publishing them.
    static IMPORT_DEFER_JOB: Uuid;
}

/// Runs `fut` with event deferral to `job_id` (source `deferEvents`).
pub async fn defer_import_events<F: std::future::Future>(job_id: Uuid, fut: F) -> F::Output {
    IMPORT_DEFER_JOB.scope(job_id, fut).await
}

/// Begins a read-only transaction on one REPEATABLE READ snapshot, for reads
/// that check the credential and permission and then load data. Every
/// statement sees the same snapshot, so the permission check covers the rows
/// returned without locking the project row: the read never waits on (or
/// blocks) project writers and collab appends, and it never takes a
/// transaction id, which would hold back the `pg_snapshot_xmin` gate of SSE and
/// outbox readers. A row lock added to such a read fails instead of blocking.
pub async fn begin_read(pool: &PgPool) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    pool.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .await
}

pub async fn set_tenant(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut **tx)
        .await?;
    if let Ok(job_id) = IMPORT_DEFER_JOB.try_with(|job| *job) {
        sqlx::query("SELECT set_config('app.import_defer_job', $1, true)")
            .bind(job_id.to_string())
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

pub async fn set_system(tx: &mut Transaction<'_, Postgres>) -> Result<String, sqlx::Error> {
    let previous: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.system_ctx', true)")
            .fetch_one(&mut **tx)
            .await?;
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut **tx)
        .await?;
    Ok(previous.unwrap_or_default())
}

pub async fn restore_system(
    tx: &mut Transaction<'_, Postgres>,
    previous: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.system_ctx', $1, true)")
        .bind(previous)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn set_self_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn clear_self_user(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.self_user_id', '', true)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn set_invitation_token_hash(
    tx: &mut Transaction<'_, Postgres>,
    token_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.invitation_token_hash', $1, true)")
        .bind(token_hash)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn clear_invitation_token_hash(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.invitation_token_hash', '', true)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub fn lock_key_from_uuid(id: Uuid) -> i32 {
    let hex = id.simple().to_string();
    let tail = hex.chars().rev().take(8).collect::<Vec<_>>();
    let tail: String = tail.into_iter().rev().collect();
    let parsed = u32::from_str_radix(&tail, 16).unwrap_or(0);
    parsed as i32
}

pub async fn lock_membership_users(
    tx: &mut Transaction<'_, Postgres>,
    user_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut keys = user_ids
        .iter()
        .map(|id| lock_key_from_uuid(*id))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
            .bind(MEMBERSHIP_LOCK_NAMESPACE)
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

const CREDENTIAL_LIVE_SQL: &str = r#"
        SELECT (
            u.deleted_at IS NULL
            AND u.suspended_at IS NULL
            AND (
                (
                    s.id IS NOT NULL
                    AND s.revoked_at IS NULL
                    AND s.expires_at > clock_timestamp()
                )
                OR (
                    t.id IS NOT NULL
                    AND t.user_id = u.id
                    AND (t.expires_at IS NULL OR t.expires_at > clock_timestamp())
                )
            )
        )
        FROM fvoci.users u
        LEFT JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        LEFT JOIN fvoci.api_tokens t ON t.id = $2 AND t.user_id = u.id
        WHERE u.id = $1
        "#;

const SESSION_RECHECK_SQL: &str = r#"
        SELECT (
            u.deleted_at IS NULL
            AND u.suspended_at IS NULL
            AND s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        FOR UPDATE OF u, s
        "#;

const TOKEN_RECHECK_SQL: &str = r#"
        SELECT (
            u.deleted_at IS NULL
            AND u.suspended_at IS NULL
            AND t.user_id = u.id
            AND (t.expires_at IS NULL OR t.expires_at > clock_timestamp())
        )
        FROM fvoci.users u
        INNER JOIN fvoci.api_tokens t ON t.id = $2 AND t.user_id = u.id
        WHERE u.id = $1
        FOR UPDATE OF u, t
        "#;

pub async fn recheck_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let session_live: Option<(bool,)> = sqlx::query_as(SESSION_RECHECK_SQL)
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some((live,)) = session_live {
        return Ok(live);
    }
    let token_live: Option<(bool,)> = sqlx::query_as(TOKEN_RECHECK_SQL)
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(token_live.map(|(v,)| v).unwrap_or(false))
}

pub async fn session_is_live(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(CREDENTIAL_LIVE_SQL)
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

pub async fn lock_tree(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TREE_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(workspace_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unique<T: Copy + Eq + std::fmt::Debug>(space: &str, locks: &[(&str, T)]) {
        let mut shared = Vec::new();
        for (i, (name, value)) in locks.iter().enumerate() {
            for (other, other_value) in &locks[i + 1..] {
                if value == other_value {
                    shared.push(format!("{name} and {other} share {value:?}"));
                }
            }
        }
        assert!(shared.is_empty(), "{space} advisory locks: {shared:?}");
    }

    /// Two users of one `(namespace, key)` advisory lock namespace contend
    /// whenever their 32-bit keys match: a session lock (room fence, job
    /// claim, attachment upload) then stalls every transaction lock of the
    /// other user on that key. Each namespace needs its own value. The
    /// one-bigint key space is separate from the two-int space.
    #[test]
    fn advisory_lock_namespaces_are_unique() {
        assert_unique(
            "two-int",
            &[
                ("attachment", crate::attachments::ATTACHMENT_LOCK_NAMESPACE),
                ("storage", crate::attachments::STORAGE_LOCK_NAMESPACE),
                ("task status", crate::db::tasks::TASK_STATUS_LOCK_NAMESPACE),
                ("collab init", crate::db::collab::COLLAB_INIT_LOCK_NAMESPACE),
                ("tree", TREE_LOCK_NAMESPACE),
                ("membership", MEMBERSHIP_LOCK_NAMESPACE),
                (
                    "collab room fence",
                    crate::db::collab::COLLAB_ROOM_SESSION_LOCK_NAMESPACE,
                ),
                ("search index", SEARCH_INDEX_LOCK_NAMESPACE),
                ("job claim", crate::jobs::JOB_LOCK_NAMESPACE),
                (
                    "task origin",
                    crate::db::task_origins::TASK_ORIGIN_LOCK_NAMESPACE,
                ),
                (
                    "github issue",
                    crate::integrations::github::ISSUE_LOCK_NAMESPACE,
                ),
            ],
        );
        // db::legal also takes one-bigint locks, keyed by an int4
        // hashtext('fvoci.legal:' || kind); they cannot be listed here.
        assert_unique(
            "one-bigint",
            &[
                ("search rebuild", SEARCH_REBUILD_LOCK_KEY),
                ("admission", crate::db::quota::ADMISSION_LOCK_KEY),
                (
                    "instance admin",
                    crate::db::identity::INSTANCE_ADMIN_LOCK_KEY,
                ),
                // prepare holds its session lock while migrate takes the
                // migration lock on other connections: equal values would
                // deadlock prepare.
                ("migration", crate::db::migrate::MIGRATION_LOCK_KEY),
                ("prepare", crate::prepare::PREPARE_LOCK_KEY),
            ],
        );
    }
}
