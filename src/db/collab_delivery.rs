//! Non-locking current-authority snapshot for collab *outbound* delivery.
//!
//! Write admission, `append_collab_update`, and join still use the locking
//! path in `db::collab`. This module is the last-chance recheck before a
//! transport socket write of a Data frame.
//!
//! ## Snapshot point
//! `check_delivery_admission` runs one READ COMMITTED `SELECT` that joins the
//! current user, session, workspace, membership, and wiki document in a tenant
//! transaction. `set_tenant` is SET LOCAL; the snapshot is the SELECT's, not
//! sequential statements. There are no row or advisory locks, so a writer
//! holding `FOR UPDATE` does not block this read (R2).
//!
//! The pool must be the PostgreSQL primary. A replica lag would make a
//! post-commit revoke invisible and would break the after-commit argument.
//!
//! ## Already in-flight frames (not recalled)
//! The room actor enqueues bounded frames without this query. Authority is
//! applied when the transport dequeues a Data frame. Transport captures one
//! `tokio::time::Instant` deadline at dequeue (`outbound_send_deadline_ms`
//! from that instant). The delivery read and the Data-frame socket write both
//! use `timeout_at` / remaining time against **that same Instant**. They do
//! not each get a fresh full `outbound_send_deadline_ms`. A read whose
//! statement started before revoke commit may still return Allowed; that
//! frame may be written after t_R only if both the read and the write finish
//! before the shared deadline (R1). Frames already inside `send_ws_message`
//! are not recalled. Data is never written after Denied, DbError, timeout, or
//! cancel during that read.
//!
//! Close after those Data-path failures is **not** leftover time from the
//! Data deadline. An expired dequeue Instant cannot deliver 1011. Transport
//! writes that Close with a separate best-effort cleanup grace of
//! `min(outbound_send_deadline_ms, 250ms)` measured from `Instant::now()` at
//! the Close attempt. That 250ms cap is the documented bound; it is not a
//! claim that every closing path fits inside an already-expired Data budget.
//!
//! Idle sockets with an empty outbound queue wait for `poll_acl` (up to
//! `revoke_poll_ms`). That delay is not authorization of new data.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::set_tenant;
use crate::db::documents::wiki_can_edit;
use crate::db::workspace::WorkspaceRole;

/// Result of the single-statement delivery read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAdmission {
    Allowed { read_only: bool },
    Denied,
}

/// Mapped outcome for transport close codes: Denied → 1008, sqlx/timeout → 1011.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundDeliveryAuth {
    Allowed { read_only: bool },
    Denied,
    DbError,
}

#[cfg(feature = "db-tests")]
static DELIVERY_READ_COUNTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
static FORCE_DELIVERY_READ_FAIL: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
pub fn delivery_read_count(document_id: Uuid) -> usize {
    DELIVERY_READ_COUNTS
        .lock()
        .map(|counts| counts.get(&document_id).copied().unwrap_or(0))
        .unwrap_or(0)
}

#[cfg(feature = "db-tests")]
pub fn reset_delivery_read_count(document_id: Uuid) {
    if let Ok(mut counts) = DELIVERY_READ_COUNTS.lock() {
        counts.insert(document_id, 0);
    }
}

#[cfg(feature = "db-tests")]
pub fn arm_force_delivery_read_fail(document_id: Uuid) {
    if let Ok(mut set) = FORCE_DELIVERY_READ_FAIL.lock() {
        set.insert(document_id);
    }
}

#[cfg(feature = "db-tests")]
pub fn disarm_force_delivery_read_fail(document_id: Uuid) {
    if let Ok(mut set) = FORCE_DELIVERY_READ_FAIL.lock() {
        set.remove(&document_id);
    }
}

#[cfg(feature = "db-tests")]
struct DeliveryReadBarrier {
    reached_tx: tokio::sync::oneshot::Sender<()>,
    proceed_rx: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(feature = "db-tests")]
static DELIVERY_READ_BARRIERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, DeliveryReadBarrier>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
static FORCE_DELIVERY_TX_ERROR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// Pause outbound delivery authorization after `set_tenant` for `session_id`.
/// Periodic actor ACL sweeps must not consume this transport-only barrier.
#[cfg(feature = "db-tests")]
pub fn arm_delivery_read_barrier(
    session_id: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    DELIVERY_READ_BARRIERS
        .lock()
        .expect("delivery read barriers")
        .insert(
            session_id,
            DeliveryReadBarrier {
                reached_tx,
                proceed_rx,
            },
        );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub fn disarm_delivery_read_barrier(session_id: Uuid) {
    if let Ok(mut barriers) = DELIVERY_READ_BARRIERS.lock() {
        barriers.remove(&session_id);
    }
}

#[cfg(feature = "db-tests")]
async fn pause_for_delivery_read_barrier(session_id: Uuid) {
    let barrier = DELIVERY_READ_BARRIERS
        .lock()
        .ok()
        .and_then(|mut barriers| barriers.remove(&session_id));
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
pub fn arm_force_delivery_tx_error(session_id: Uuid) {
    if let Ok(mut set) = FORCE_DELIVERY_TX_ERROR.lock() {
        set.insert(session_id);
    }
}

#[cfg(feature = "db-tests")]
pub fn disarm_force_delivery_tx_error(session_id: Uuid) {
    if let Ok(mut set) = FORCE_DELIVERY_TX_ERROR.lock() {
        set.remove(&session_id);
    }
}

#[cfg(feature = "db-tests")]
fn take_force_delivery_tx_error(session_id: Uuid) -> bool {
    FORCE_DELIVERY_TX_ERROR
        .lock()
        .map(|mut set| set.remove(&session_id))
        .unwrap_or(false)
}

type DeliveryRow = (
    bool,
    bool,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<DateTime<Utc>>,
);
pub async fn check_delivery_admission(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<DeliveryAdmission, sqlx::Error> {
    check_delivery_admission_inner(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        #[cfg(feature = "db-tests")]
        false,
    )
    .await
}

async fn check_delivery_admission_inner(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    #[cfg(feature = "db-tests")] outbound_barrier: bool,
) -> Result<DeliveryAdmission, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *tx)
        .await?;
    set_tenant(&mut tx, workspace_id).await?;
    #[cfg(feature = "db-tests")]
    if outbound_barrier {
        pause_for_delivery_read_barrier(session_id).await;
    }
    #[cfg(feature = "db-tests")]
    if take_force_delivery_tx_error(session_id) {
        sqlx::query("SELECT 1 / 0").execute(&mut *tx).await?;
    }
    let row: Option<DeliveryRow> = sqlx::query_as(
        r#"
        SELECT
            (
                s.revoked_at IS NULL
                AND s.expires_at > clock_timestamp()
                AND u.deleted_at IS NULL
                AND u.suspended_at IS NULL
            ) AS session_live,
            (w.deleted_at IS NULL) AS workspace_live,
            m.role,
            d.project_id,
            d.status,
            d.deleted_at
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        INNER JOIN fvoci.workspaces w ON w.id = $3
        LEFT JOIN fvoci.memberships m
            ON m.workspace_id = $3 AND m.user_id = u.id
        LEFT JOIN fvoci.documents d
            ON d.workspace_id = $3 AND d.id = $4
        WHERE u.id = $1
        "#,
    )
    .bind(actor_user_id)
    .bind(session_id)
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    let Some((session_live, workspace_live, role, project_id, status, deleted_at)) = row else {
        return Ok(DeliveryAdmission::Denied);
    };
    if !session_live || !workspace_live {
        return Ok(DeliveryAdmission::Denied);
    }
    let role = role.as_deref().and_then(WorkspaceRole::parse);
    if !wiki_can_edit(role) {
        return Ok(DeliveryAdmission::Denied);
    }
    let Some(status) = status else {
        return Ok(DeliveryAdmission::Denied);
    };
    if deleted_at.is_some() || project_id.is_some() {
        return Ok(DeliveryAdmission::Denied);
    }
    Ok(DeliveryAdmission::Allowed {
        read_only: status == "archived",
    })
}

/// Fail-closed wrapper: sqlx errors become [`OutboundDeliveryAuth::DbError`].
pub async fn authorize_outbound_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> OutboundDeliveryAuth {
    #[cfg(feature = "db-tests")]
    {
        if let Ok(mut counts) = DELIVERY_READ_COUNTS.lock() {
            *counts.entry(document_id).or_insert(0) += 1;
        }
        let forced = FORCE_DELIVERY_READ_FAIL
            .lock()
            .map(|set| set.contains(&document_id))
            .unwrap_or(false);
        if forced {
            return OutboundDeliveryAuth::DbError;
        }
    }
    match check_delivery_admission_inner(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        #[cfg(feature = "db-tests")]
        true,
    )
    .await
    {
        Ok(DeliveryAdmission::Allowed { read_only }) => OutboundDeliveryAuth::Allowed { read_only },
        Ok(DeliveryAdmission::Denied) => OutboundDeliveryAuth::Denied,
        Err(_) => OutboundDeliveryAuth::DbError,
    }
}
