//! Transaction context (tenant, system, self user, invitation token) and the
//! credential and advisory-lock checks request transactions run.
//!
//! Every setting is transaction-local (`set_config(.., true)`), so a pooled
//! connection never carries one into its next checkout.
//!
//! Order for workspace-scoped request transactions. Writers: [`set_tenant`]
//! → [`lock_membership_users`] → [`recheck_session`] → (only where the write
//! depends on the actor's workspace role)
//! `db::workspace::membership_role_for_update` → parent rows before child
//! rows. Writers that admit members, change a member's role, or create or
//! delete workspaces first take `quota::acquire_admission_lock`; writers that
//! change instance users take `identity::lock_instance_admin_changes`
//! (admission, then instance-admin). Both come before
//! [`lock_membership_users`], and `identity::lock_sign_in` comes after it.
//! Module-specific tails are documented in `db::collab`, `db::task_ops` and
//! `db::project_documents`.
//!
//! Reads check the credential and permission in the same transaction as the
//! data. Whether they take row locks depends on the check they reuse, not on
//! being a read:
//! - [`session_is_live`] (the usual read check) and [`begin_read`] (one
//!   REPEATABLE READ, READ ONLY snapshot, used by most project-scoped reads)
//!   take no row locks. Stream access checks use a plain transaction with the
//!   same non-locking checks.
//! - A read that reuses the writers' prefix locks what that prefix locks:
//!   [`recheck_session`] holds the actor's users and session (or token) rows
//!   `FOR UPDATE` (for example the admin-only import job, workspace SSO
//!   config and workspace export reads), and the collab reads in `db::collab`
//!   (room admission, operation lookup and verify, read-only load) run
//!   `lock_collab_actor` (steps 1-5 of that module's lock order); the
//!   read-only load also locks, and may seed, the state row.
//! - `db::legal::workspace_consents` holds the live workspace row `FOR SHARE`
//!   and leaves the credential check to its route.
//! - Instance-admin reads use `db::admin::require_live_instance_admin`, which
//!   holds the actor's users row `FOR SHARE`: `db::admin` reads run it in the
//!   reading transaction, and the instance-settings read route runs it in a
//!   transaction of its own before and again after loading the settings.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

// Advisory-lock namespaces: two-int locks are (namespace, key), one-bigint
// locks a separate key space. Every fixed namespace and bigint key in the
// crate is listed in `tests::advisory_lock_namespaces_are_unique`, which
// states why each must be unique. Values may be renumbered: servers of different
// versions never run against one database (RUNNING.md: mixed-version rolling
// restart is not supported).
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

/// The low 32 bits of `id`, the key of a two-int advisory lock on that id.
/// Every process must derive the same key for the same id (the room fence and
/// the tests that hold its keys depend on it). Equal keys never grant access,
/// but they make unrelated ids contend: a transaction lock waits, and a
/// try-lock (room fence, attachment upload) reports busy for as long as the
/// holder keeps it.
pub fn lock_key_from_uuid(id: Uuid) -> i32 {
    let hex = id.simple().to_string();
    let tail = hex.chars().rev().take(8).collect::<Vec<_>>();
    let tail: String = tail.into_iter().rev().collect();
    let parsed = u32::from_str_radix(&tail, 16).unwrap_or(0);
    parsed as i32
}

/// Takes the transaction-scoped membership advisory lock of each user, keys
/// sorted and deduplicated, one statement per key. Writers take it for the
/// actor before [`recheck_session`]; membership, admin and account changes
/// take it for every affected user, so those changes and the user's writes
/// serialize. The fixed key order keeps transactions that lock overlapping
/// user sets deadlock-free.
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

/// Write-path credential check: the same condition as [`session_is_live`],
/// but it locks the user row and the session or API token row `FOR UPDATE`,
/// so a revocation, suspension or deletion of those rows either committed
/// first (and is seen) or waits until this transaction ends. Session first,
/// then token, as two statements: `FOR UPDATE` cannot lock the nullable side
/// of an outer join. Call it after [`lock_membership_users`]; the tenant rule
/// of [`session_is_live`] applies.
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

/// Whether `session_id` names a live credential of a live user: an unrevoked,
/// unexpired session or an unexpired API token, of a user neither deleted nor
/// suspended. Takes no lock (read path; writers use [`recheck_session`]).
///
/// Run after [`set_tenant`]: `api_tokens` has RLS, so with no tenant a live
/// token reads as dead, and under system context another workspace's token
/// would be visible. The route-level binding of a token to its workspace is
/// `http::authz::apply_token_access`; this is its second layer.
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
