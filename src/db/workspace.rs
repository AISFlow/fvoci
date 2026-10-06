use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, OperationTx};
use super::codec::Cell;

use crate::db::context::{
    clear_self_user, lock_membership_users, recheck_session, session_is_live, set_self_user,
    set_tenant,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{self, visible_project_sql};
use crate::db::quota::{
    acquire_admission_lock, require_membership_admission, require_new_instance_billable_user,
    QuotaError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
    Guest,
}

impl WorkspaceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Guest => "guest",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "guest" => Some(Self::Guest),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Guest => 0,
            Self::Member => 1,
            Self::Admin => 2,
            Self::Owner => 3,
        }
    }

    pub fn at_least(self, min: Self) -> bool {
        self.rank() >= min.rank()
    }
}

#[derive(Debug)]
pub enum WorkspaceDbError {
    NotFound,
    Forbidden,
    PersonalImmutable,
    LastOwner,
    SelfChange,
    RoleCap,
    SlugTaken,
    LastProjectLead,
    SeatLimit,
    GuestLimit,
    InvalidInput,
}

pub struct WorkspaceListItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: WorkspaceRole,
    pub kind: String,
    pub document_count: i32,
    pub assigned_count: i32,
}

pub struct WorkspaceMeta {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

pub struct MemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub role: WorkspaceRole,
}

fn quota_error(err: QuotaError) -> WorkspaceDbError {
    match err {
        QuotaError::SeatLimit => WorkspaceDbError::SeatLimit,
        QuotaError::GuestLimit => WorkspaceDbError::GuestLimit,
    }
}

pub fn personal_workspace_slug(user_id: Uuid) -> String {
    let hex = user_id.simple().to_string();
    let tail = hex.chars().rev().take(12).collect::<String>();
    format!("u-{}", tail.chars().rev().collect::<String>())
}

async fn user_is_active(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(active,)| active).unwrap_or(false))
}

pub(crate) struct WorkspaceChangeRecord<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub verb: &'a str,
    pub target_type: &'a str,
    pub target_id: Uuid,
    pub payload: serde_json::Value,
    pub client_ip: Option<&'a str>,
}

pub(crate) async fn record_workspace_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: WorkspaceChangeRecord<'_>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload,
            ip: change.client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn membership_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

pub(crate) async fn membership_role_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

pub(crate) async fn workspace_is_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(live,)| live).unwrap_or(false))
}

impl OperationTx<'_, '_> {
    pub(crate) async fn membership_role(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        for_update: bool,
    ) -> Result<Option<WorkspaceRole>, sqlx::Error> {
        match self {
            Self::Postgres(tx) if for_update => {
                membership_role_for_update(tx, workspace, user).await
            }
            Self::Postgres(tx) => membership_role(tx, workspace, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                if for_update {
                    tx.require_writer()?;
                }
                let rows = tx
                    .query(
                        "SELECT role FROM memberships WHERE workspace_id=?1 AND user_id=?2",
                        &[Cell::uuid(workspace), Cell::uuid(user)],
                    )
                    .await?;
                let role = rows.first().map(|row| row.cell(0)?.string()).transpose()?;
                Ok(role.as_deref().and_then(WorkspaceRole::parse))
            }
        }
    }

    pub(crate) async fn workspace_is_live(&mut self, workspace: Uuid) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => workspace_is_live(tx, workspace).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows = tx
                    .query(
                        "SELECT deleted_at IS NULL FROM workspaces WHERE id=?1",
                        &[Cell::uuid(workspace)],
                    )
                    .await?;
                Ok(rows
                    .first()
                    .map(|row| row.cell(0)?.boolean())
                    .transpose()?
                    .unwrap_or(false))
            }
        }
    }
}

pub(crate) async fn workspace_kind_read(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match row {
        Some((kind, deleted)) if deleted.is_none() => Ok(kind),
        _ => Ok(None),
    }
}

async fn workspace_kind(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match row {
        Some((kind, deleted)) if deleted.is_none() => Ok(kind),
        _ => Ok(None),
    }
}

#[cfg(feature = "db-tests")]
type WorkspaceCardBarriers = std::collections::HashMap<
    Uuid,
    (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ),
>;

#[cfg(feature = "db-tests")]
static WORKSPACE_CARD_BARRIERS: std::sync::LazyLock<tokio::sync::Mutex<WorkspaceCardBarriers>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

/// Pause this actor only, after the self-list transaction has actually committed
/// and before any card transaction reserves its writer/current authority.
#[cfg(feature = "db-tests")]
pub async fn arm_workspace_card_barrier(
    user: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    assert!(WORKSPACE_CARD_BARRIERS
        .lock()
        .await
        .insert(user, (reached_tx, proceed_rx))
        .is_none());
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
async fn pause_before_workspace_cards(user: Uuid) {
    let barrier = WORKSPACE_CARD_BARRIERS.lock().await.remove(&user);
    if let Some((reached, proceed)) = barrier {
        let _ = reached.send(());
        let _ = proceed.await;
    }
}

pub async fn list_workspaces_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceListItem>, sqlx::Error> {
    list_workspaces_for_user_backend(&Backend::Postgres(pool.clone()), user_id).await
}

pub async fn list_workspaces_for_user_backend(
    backend: &Backend,
    user_id: Uuid,
) -> Result<Vec<WorkspaceListItem>, sqlx::Error> {
    // Retain the PG transaction boundaries/isolation and self-user RLS prefix.
    let mut tx = backend.begin_write().await?;
    let memberships = tx.operation().self_workspace_memberships(user_id).await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    #[cfg(feature = "db-tests")]
    pause_before_workspace_cards(user_id).await;
    let mut items = Vec::new();
    for (workspace_id, _) in memberships {
        let mut tx = backend.begin_write().await?;
        tx.operation().set_tenant(workspace_id).await?;
        // Enumeration only identifies candidates. Hold current membership
        // authority through this card's read/count transaction.
        let Some(role) = tx
            .operation()
            .membership_role(workspace_id, user_id, true)
            .await?
        else {
            tx.rollback().await?;
            continue;
        };
        let row = tx.operation().live_workspace_card(workspace_id).await?;
        if let Some((id, name, slug, kind)) = row {
            let (document_count, assigned_count) = tx
                .operation()
                .workspace_card_counts(workspace_id, user_id, role)
                .await?;
            items.push(WorkspaceListItem {
                id,
                name,
                slug,
                role,
                kind,
                document_count,
                assigned_count,
            });
        }
        tx.commit().await.map_err(|unknown| unknown.source)?;
    }
    Ok(items)
}

impl OperationTx<'_, '_> {
    async fn self_workspace_memberships(
        &mut self,
        user: Uuid,
    ) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                set_self_user(tx, user).await?;
                let rows = sqlx::query_as(
                    "SELECT workspace_id, role FROM fvoci.memberships WHERE user_id=$1",
                )
                .bind(user)
                .fetch_all(&mut ***tx)
                .await?;
                clear_self_user(tx).await?;
                Ok(rows)
            }
            Self::SqliteFamily(tx) => {
                // This named self read carries its actor explicitly; it cannot
                // enumerate another user's rows through an unscoped query.
                tx.query(
                    "SELECT workspace_id,role FROM memberships WHERE user_id=?1",
                    &[Cell::uuid(user)],
                )
                .await?
                .iter()
                .map(|r| Ok((r.cell(0)?.id()?, r.cell(1)?.string()?)))
                .collect()
            }
        }
    }
    async fn live_workspace_card(
        &mut self,
        workspace: Uuid,
    ) -> Result<Option<(Uuid, String, String, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT id,name,slug,kind FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(workspace)
            .fetch_optional(&mut ***tx)
            .await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT id,name,slug,kind FROM workspaces WHERE id=?1 AND deleted_at IS NULL", &[Cell::uuid(workspace)]).await?;
                rows.first()
                    .map(|r| {
                        Ok((
                            r.cell(0)?.id()?,
                            r.cell(1)?.string()?,
                            r.cell(2)?.string()?,
                            r.cell(3)?.string()?,
                        ))
                    })
                    .transpose()
            }
        }
    }
    async fn workspace_card_counts(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        role: WorkspaceRole,
    ) -> Result<(i32, i32), sqlx::Error> {
        match self {
            Self::Postgres(tx) => workspace_card_counts_in_tx(tx, workspace, user, role).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query(
                    r#"WITH visible_projects AS (
                        SELECT p.id FROM projects p
                        WHERE p.workspace_id=?1 AND p.deleted_at IS NULL
                          AND ((p.visibility='workspace' AND ?2=0)
                            OR EXISTS(SELECT 1 FROM project_members pm
                                      WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND pm.user_id=?3)
                            OR EXISTS(SELECT 1 FROM project_members pm
                                      INNER JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id
                                      WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND gm.user_id=?3 AND pm.group_id IS NOT NULL))
                    )
                    SELECT
                      (SELECT count(*) FROM documents d
                       WHERE d.workspace_id=?1 AND d.deleted_at IS NULL
                         AND ((d.project_id IS NULL AND ?2=0) OR d.project_id IN (SELECT id FROM visible_projects))),
                      (SELECT count(*) FROM tasks t
                       WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
                         AND t.project_id IN (SELECT id FROM visible_projects)
                         AND EXISTS(SELECT 1 FROM task_assignees a WHERE a.workspace_id=t.workspace_id AND a.task_id=t.id AND a.user_id=?3)
                         AND EXISTS(SELECT 1 FROM statuses s_open WHERE s_open.workspace_id=t.workspace_id AND s_open.project_id=t.project_id AND s_open.id=t.status_id AND s_open.category NOT IN ('done','canceled')))
                    "#,
                    &[Cell::uuid(workspace),Cell::Integer(i64::from(role==WorkspaceRole::Guest)),Cell::uuid(user)]
                ).await?;
                let row = rows
                    .first()
                    .ok_or_else(|| sqlx::Error::Protocol("workspace card counts absent".into()))?;
                let documents = row.cell(0)?.integer()?;
                let assigned = row.cell(1)?.integer()?;
                if documents < 0 || assigned < 0 {
                    return Err(sqlx::Error::Protocol("negative workspace count".into()));
                }
                Ok((
                    i32::try_from(documents).unwrap_or(i32::MAX),
                    i32::try_from(assigned).unwrap_or(i32::MAX),
                ))
            }
        }
    }
}

pub async fn list_members(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Member))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if workspace_kind_read(&mut tx, workspace_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, m.role
        FROM fvoci.memberships m
        INNER JOIN fvoci.users u ON u.id = m.user_id
        WHERE m.workspace_id = $1 AND u.deleted_at IS NULL
        ORDER BY m.created_at ASC, u.id ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(user_id, email, given_name, family_name, role)| {
            WorkspaceRole::parse(&role).map(|role| MemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        })
        .collect()))
}

/// Selected member read with the original actor/session/member threshold and
/// live-user privacy. PG keeps its original public transaction/RLS contract.
pub async fn list_members_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_members(pool, workspace_id, actor_user_id, session_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        tx.operation().set_tenant(workspace_id).await?;
        tx.operation()
            .authorized_workspace_members(workspace_id, actor_user_id, session_id)
            .await
    }
    .await;
    let cleanup = tx.rollback().await;
    member_read_after_rollback(result, cleanup)
}

// WorkspaceDbError is the existing public domain result, not an Error. This
// private envelope retains that exact refusal only when actual cleanup fails.
#[derive(Debug, thiserror::Error)]
#[error("workspace member read refused: {0:?}")]
struct MemberReadRefusal(WorkspaceDbError);

fn member_read_after_rollback(
    result: Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(MemberReadRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(super::backend::rollback_cleanup_unknown(original, cleanup))
        }
    }
}

#[cfg(test)]
mod selected_member_read_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;

    #[tokio::test]
    async fn selected_member_read_synthetic_cleanup_error_after_real_rollback_retains_causes() {
        let f = Fixture::new().await;
        for domain in [true, false] {
            let mut tx = f.backend.begin_read().await.unwrap();
            tx.operation()
                .set_tenant(if domain {
                    f.workspace
                } else {
                    f.other_workspace
                })
                .await
                .unwrap();
            let result = tx
                .operation()
                .authorized_workspace_members(f.workspace, f.user, f.credential)
                .await;
            if domain {
                assert!(matches!(&result, Ok(Err(WorkspaceDbError::Forbidden))));
            } else {
                assert!(matches!(&result, Err(sqlx::Error::Protocol(_))));
            }
            // Actual local rollback is acknowledged first. The following
            // synthetic returned error tests propagation only; it is not a
            // provider cleanup failure or evidence of remote settlement.
            tx.rollback().await.unwrap();
            let error = member_read_after_rollback(
                result,
                Err(sqlx::Error::Protocol(
                    "synthetic cleanup error after acknowledged real rollback".into(),
                )),
            )
            .err()
            .expect("uncertain cleanup must be an outer typed error");
            assert!(crate::db::backend::is_rollback_cleanup_unknown(&error));
            let sqlx::Error::AnyDriverError(source) = &error else {
                panic!("canonical cleanup receipt required")
            };
            let receipt = source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .expect("shared receipt must survive the public SQLx result");
            assert!(matches!(&receipt.cleanup, sqlx::Error::Protocol(message)
                if message == "synthetic cleanup error after acknowledged real rollback"));
            let original = receipt
                .original
                .as_ref()
                .expect("original refusal retained");
            if domain {
                let original = original.downcast_ref::<MemberReadRefusal>().unwrap();
                assert!(matches!(&original.0, WorkspaceDbError::Forbidden));
            } else {
                assert!(matches!(
                    original.downcast_ref::<sqlx::Error>(),
                    Some(sqlx::Error::Protocol(_))
                ));
            }
        }
        assert!(matches!(
            list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        sqlx::query("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let rows = list_members_backend(&f.backend, f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rows.len(),
            2,
            "known healthy rollback preserves the public read result"
        );
        assert!(rows.iter().any(|row| row.user_id == f.actor));
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }

    #[tokio::test]
    async fn selected_member_read_current_credential_role_live_users_and_order() {
        let f = Fixture::new().await;
        assert!(
            matches!(
                list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                    .await
                    .unwrap(),
                Err(WorkspaceDbError::Forbidden)
            ),
            "guest cannot enumerate member email identities"
        );
        sqlx::query("UPDATE memberships SET role='member',created_at=100 WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE memberships SET created_at=200 WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let rows = list_members_backend(&f.backend, f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.user_id).collect::<Vec<_>>(),
            vec![f.user, f.actor]
        );
        assert_eq!(rows[1].email, "actor@notification.invalid");
        assert_eq!(rows[1].role, WorkspaceRole::Owner);
        assert_eq!(rows[0].role, WorkspaceRole::Member);
        assert_eq!(rows[0].given_name, "한글🙂");
        assert!(matches!(
            list_members_backend(&f.backend, f.workspace, f.user, f.other_credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        assert!(matches!(
            list_members_backend(&f.backend, f.other_workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        sqlx::query("UPDATE users SET deleted_at=1 WHERE id=?1")
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let rows = list_members_backend(&f.backend, f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.user_id).collect::<Vec<_>>(),
            vec![f.user],
            "retained memberships cannot expose withdrawn user's identity"
        );
        for column in ["suspended_at", "deleted_at"] {
            let sql = match column {
                "suspended_at" => "UPDATE users SET suspended_at=1 WHERE id=?1",
                _ => "UPDATE users SET deleted_at=1 WHERE id=?1",
            };
            sqlx::query(sql)
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert!(matches!(
                list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                    .await
                    .unwrap(),
                Err(WorkspaceDbError::Forbidden)
            ));
            sqlx::query("UPDATE users SET suspended_at=NULL,deleted_at=NULL WHERE id=?1")
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET expires_at=?2 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .bind(chrono::Utc::now().timestamp_micros() + 3_600_000_000i64)
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let _ = tx
            .operation()
            .authorized_workspace_members(f.workspace, f.user, f.credential)
            .await
            .unwrap()
            .unwrap();
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            panic!("actual SQLite transaction required")
        };
        assert!(
            family.require_system_context().is_err(),
            "member read cannot broaden system scope"
        );
        tx.rollback().await.unwrap();
        let mut wrong = f.backend.begin_read().await.unwrap();
        wrong
            .operation()
            .set_tenant(f.other_workspace)
            .await
            .unwrap();
        assert!(wrong
            .operation()
            .authorized_workspace_members(f.workspace, f.user, f.credential)
            .await
            .is_err());
        wrong.rollback().await.unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_members_backend(&f.backend, f.workspace, f.user, f.credential)
                .await
                .unwrap(),
            Err(WorkspaceDbError::NotFound)
        ));
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
}

impl OperationTx<'_, '_> {
    pub(crate) async fn authorized_workspace_members(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
    ) -> Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error> {
        if !self.session_is_live(actor, credential).await?
            || !self
                .membership_role(workspace, actor, false)
                .await?
                .is_some_and(|role| role.at_least(WorkspaceRole::Member))
        {
            return Ok(Err(WorkspaceDbError::Forbidden));
        }
        if !self.workspace_is_live(workspace).await? {
            return Ok(Err(WorkspaceDbError::NotFound));
        }
        let rows: Vec<(Uuid,String,String,Option<String>,String)> = match self {
            Self::Postgres(tx) => sqlx::query_as("SELECT u.id,u.email,u.given_name,u.family_name,m.role FROM fvoci.memberships m INNER JOIN fvoci.users u ON u.id=m.user_id WHERE m.workspace_id=$1 AND u.deleted_at IS NULL ORDER BY m.created_at ASC,u.id ASC")
                .bind(workspace).fetch_all(&mut ***tx).await?,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.query("SELECT u.id,u.email,u.given_name,u.family_name,m.role FROM memberships m INNER JOIN users u ON u.id=m.user_id WHERE m.workspace_id=?1 AND u.deleted_at IS NULL ORDER BY m.created_at,u.id", &[Cell::uuid(workspace)]).await?
                    .iter().map(|row|Ok((row.cell(0)?.id()?,row.cell(1)?.string()?,row.cell(2)?.string()?,row.cell(3)?.optional(Cell::string)?,row.cell(4)?.string()?))).collect::<Result<_,sqlx::Error>>()?
            }
        };
        Ok(Ok(rows
            .into_iter()
            .filter_map(|(user_id, email, given_name, family_name, role)| {
                WorkspaceRole::parse(&role).map(|role| MemberRow {
                    user_id,
                    email,
                    given_name,
                    family_name,
                    role,
                })
            })
            .collect()))
    }
}

pub async fn get_workspace_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    get_workspace_meta_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        actor_user_id,
        session_id,
    )
    .await
}

pub async fn get_workspace_meta_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace_id).await?;
    if !tx
        .operation()
        .session_is_live(actor_user_id, session_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = tx
        .operation()
        .membership_role(workspace_id, actor_user_id, false)
        .await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Guest))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let row = tx.operation().live_workspace_card(workspace_id).await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(match row {
        Some((id, name, slug, _)) => Ok(WorkspaceMeta { id, name, slug }),
        None => Err(WorkspaceDbError::NotFound),
    })
}

pub async fn update_workspace_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Admin))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    let previous_name: Option<(String,)> =
        sqlx::query_as("SELECT name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((from_name,)) = previous_name else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        UPDATE fvoci.workspaces
        SET name = $2, updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING id, name, slug
        "#,
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, name, slug)) = row else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "name": name,
        "fromName": from_name,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.name_updated",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta { id, name, slug }))
}

pub async fn create_workspace_as_instance_admin(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    slug: &str,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let workspace_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let admin: Option<(bool,)> = sqlx::query_as(
        "SELECT is_instance_admin FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL AND suspended_at IS NULL",
    )
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !admin.map(|(v,)| v).unwrap_or(false) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if let Err(err) =
        require_new_instance_billable_user(&mut tx, Some(actor_user_id), license).await?
    {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
    }
    set_tenant(&mut tx, workspace_id).await?;
    let inserted = sqlx::query_as::<_, (Uuid, String, String)>(
        "INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3) RETURNING id, name, slug",
    )
    .bind(workspace_id)
    .bind(slug)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await;
    if let Err(err) = inserted {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("workspaces_slug_unique") {
                tx.rollback().await?;
                return Ok(Err(WorkspaceDbError::SlugTaken));
            }
        }
        return Err(err);
    }
    let Some((id, name, slug)) = inserted? else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "ownerId": actor_user_id.to_string(),
        "name": name,
        "slug": slug,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.created",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta { id, name, slug }))
}

/// Bootstrap under the selected backend's actual admission writer. A failed
/// finish retains its original receipt and never authorizes an observer/retry.
pub async fn ensure_personal_workspace_backend(
    backend: &Backend,
    license: &crate::license::Entitlements,
    user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return ensure_personal_workspace(pool, license, user_id, session_id, client_ip).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = ensure_personal_workspace_operation(
        &mut tx.operation(),
        license,
        user_id,
        session_id,
        client_ip,
    )
    .await;
    if let Ok(Ok(meta)) = result {
        tx.commit_with_cleanup()
            .await
            .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
        Ok(Ok(meta))
    } else {
        personal_workspace_after_rollback(result, tx.rollback().await)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("personal workspace bootstrap refused: {0:?}")]
struct PersonalWorkspaceRefusal(WorkspaceDbError);

fn personal_workspace_after_rollback(
    result: Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(PersonalWorkspaceRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(super::backend::rollback_cleanup_unknown(original, cleanup))
        }
    }
}

async fn ensure_personal_workspace_operation(
    op: &mut OperationTx<'_, '_>,
    license: &crate::license::Entitlements,
    user: Uuid,
    credential: Uuid,
    ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    op.acquire_admission_lock().await?;
    let candidate = {
        let OperationTx::SqliteFamily(family) = op else {
            return Err(sqlx::Error::Protocol(
                "personal bootstrap operation requires family writer".into(),
            ));
        };
        family.require_writer()?;
        // Choose the one immutable tenant from this actor's private mapping.
        // No metadata is returned until the credential is proved below. The
        // family system marker is local, and restoration is infallible even
        // when the awaited lookup returns a driver error.
        let previous = family.replace_system_context(true);
        let result = family.query(
            "SELECT w.id FROM users u JOIN workspaces w ON w.id=u.personal_workspace_id WHERE u.id=?1 AND u.deleted_at IS NULL AND w.kind='personal' AND w.deleted_at IS NULL",
            &[Cell::uuid(user)],
        ).await;
        family.replace_system_context(previous);
        result?.first().map(|row| row.cell(0)?.id()).transpose()?
    };
    let workspace = candidate.unwrap_or_else(Uuid::now_v7);
    op.set_tenant(workspace).await?;
    op.lock_membership_users(&[user]).await?;
    if !op.recheck_session(user, credential).await? {
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if candidate.is_some() {
        let OperationTx::SqliteFamily(family) = op else {
            return Err(sqlx::Error::Protocol(
                "personal bootstrap operation requires family writer".into(),
            ));
        };
        family.require_tenant(workspace)?;
        let rows = family.query(
            "SELECT id,name,slug FROM workspaces WHERE id=?1 AND kind='personal' AND deleted_at IS NULL",
            &[Cell::uuid(workspace)],
        ).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        return Ok(Ok(WorkspaceMeta {
            id: row.cell(0)?.id()?,
            name: row.cell(1)?.string()?,
            slug: row.cell(2)?.string()?,
        }));
    }
    if let Err(error) = op
        .require_new_instance_billable_user(Some(user), license)
        .await?
    {
        return Ok(Err(quota_error(error)));
    }
    let mut slug = personal_workspace_slug(user);
    {
        let OperationTx::SqliteFamily(family) = op else {
            return Err(sqlx::Error::Protocol(
                "personal bootstrap operation requires family writer".into(),
            ));
        };
        family.require_writer()?;
        family.require_tenant(workspace)?;
        let mut inserted = false;
        for attempt in 0..4 {
            if attempt > 0 {
                let alternate = Uuid::now_v7().simple().to_string();
                slug = format!("u-{}", &alternate[4..]);
            }
            let rows = family.query(
                "INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,?2,'Personal','personal') ON CONFLICT(slug) DO NOTHING RETURNING id",
                &[Cell::uuid(workspace), Cell::text(&slug)],
            ).await?;
            if !rows.is_empty() {
                inserted = true;
                break;
            }
        }
        if !inserted {
            return Ok(Err(WorkspaceDbError::SlugTaken));
        }
        family
            .execute(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",
                &[Cell::uuid(workspace), Cell::uuid(user)],
            )
            .await?;
        if family.execute("UPDATE users SET personal_workspace_id=?2,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND deleted_at IS NULL", &[Cell::uuid(user), Cell::uuid(workspace)]).await? != 1 {
            return Err(sqlx::Error::RowNotFound);
        }
    }
    let payload = json!({"workspaceId":workspace.to_string(),"ownerId":user.to_string(),"kind":"personal","slug":slug});
    op.append_event(EventAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(user),
        verb: "workspace.personal_created".into(),
        target_type: Some("workspace".into()),
        target_id: Some(workspace),
        payload: payload.clone(),
    })
    .await?;
    op.append_audit(AuditAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(user),
        verb: "workspace.personal_created".into(),
        target_type: Some("workspace".into()),
        target_id: Some(workspace),
        payload,
        ip: ip.map(str::to_string),
    })
    .await?;
    Ok(Ok(WorkspaceMeta {
        id: workspace,
        name: "Personal".into(),
        slug,
    }))
}

pub async fn ensure_personal_workspace(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let existing: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT personal_workspace_id FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if let Some((Some(personal_id),)) = existing {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, personal_id).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, slug FROM fvoci.workspaces WHERE id = $1 AND kind = 'personal' AND deleted_at IS NULL",
        )
        .bind(personal_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((id, name, slug)) = row {
            return Ok(Ok(WorkspaceMeta { id, name, slug }));
        }
    }

    let workspace_id = Uuid::now_v7();
    let mut slug = personal_workspace_slug(user_id);
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    lock_membership_users(&mut tx, &[user_id]).await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let current: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((Some(personal_id),)) = current {
        set_tenant(&mut tx, personal_id).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, slug FROM fvoci.workspaces WHERE id = $1 AND kind = 'personal' AND deleted_at IS NULL",
        )
        .bind(personal_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, name, slug)) = row {
            tx.commit().await?;
            return Ok(Ok(WorkspaceMeta { id, name, slug }));
        }
    }
    if let Err(err) = require_new_instance_billable_user(&mut tx, Some(user_id), license).await? {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
    }
    set_tenant(&mut tx, workspace_id).await?;
    // A supported team may already own the deterministic address. Do not
    // look up, disclose or convert that tenant. Only the slug conflict is
    // handled; other constraints/errors still propagate. Existing mappings
    // and free deterministic addresses above remain stable.
    let mut inserted = false;
    for attempt in 0..4 {
        if attempt > 0 {
            let alternate = Uuid::now_v7().simple().to_string();
            slug = format!("u-{}", &alternate[4..]);
        }
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO fvoci.workspaces (id, slug, name, kind) VALUES ($1, $2, 'Personal', 'personal') ON CONFLICT (slug) DO NOTHING RETURNING id",
        )
        .bind(workspace_id)
        .bind(&slug)
        .fetch_optional(&mut *tx)
        .await?;
        if id.is_some() {
            inserted = true;
            break;
        }
    }
    if !inserted {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SlugTaken));
    }
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE fvoci.users SET personal_workspace_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "ownerId": user_id.to_string(),
        "kind": "personal",
        "slug": slug,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id: user_id,
            verb: "workspace.personal_created",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta {
        id: workspace_id,
        name: "Personal".to_string(),
        slug,
    }))
}

pub async fn set_member_role(
    db: &crate::db::Db,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    next_role: WorkspaceRole,
    client_ip: Option<&str>,
) -> Result<Result<MemberRow, WorkspaceDbError>, sqlx::Error> {
    let pool = db.pool.postgres("workspace.member_role")?;
    let license = &db.license;
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let actor_role = match actor_role {
        Some(r) if r.at_least(WorkspaceRole::Admin) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::Forbidden));
        }
    };
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if actor_user_id == target_user_id {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SelfChange));
    }
    if !user_is_active(&mut tx, target_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let target_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
    let target_role = match target_role {
        Some(r) => r,
        None => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::NotFound));
        }
    };
    if !actor_role.at_least(target_role) || !actor_role.at_least(next_role) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::RoleCap));
    }
    if target_role == WorkspaceRole::Owner
        && next_role != WorkspaceRole::Owner
        && count_owners(&mut tx, workspace_id).await? <= 1
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastOwner));
    }
    if next_role == target_role {
        tx.commit().await?;
        let member = fetch_member(pool, workspace_id, target_user_id).await?;
        return Ok(member.ok_or(WorkspaceDbError::NotFound));
    }
    if let Err(err) = require_membership_admission(
        &mut tx,
        target_user_id,
        next_role,
        Some(target_role),
        license,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
    }
    sqlx::query(
        "UPDATE fvoci.memberships SET role = $3, updated_at = now() WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(target_user_id)
    .bind(next_role.as_str())
    .execute(&mut *tx)
    .await?;
    let revoke_roles = if next_role.at_least(WorkspaceRole::Admin) {
        [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ]
        .into_iter()
        .filter(|role| !next_role.at_least(*role))
        .collect::<Vec<_>>()
    } else {
        vec![
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ]
    };
    let revoked_invitations = crate::db::invitations::remove_pending_by_inviter(
        &mut tx,
        workspace_id,
        target_user_id,
        &revoke_roles,
    )
    .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "fromRole": target_role.as_str(),
        "role": next_role.as_str(),
        "revokedInvitations": revoked_invitations,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace_member.role_changed",
            target_type: "workspace_member",
            target_id: target_user_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    let member = fetch_member(pool, workspace_id, target_user_id).await?;
    Ok(member.ok_or(WorkspaceDbError::NotFound))
}

pub async fn remove_member(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let actor_role = match actor_role {
        Some(r) if r.at_least(WorkspaceRole::Admin) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::Forbidden));
        }
    };
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if actor_user_id == target_user_id {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SelfChange));
    }
    if !user_is_active(&mut tx, target_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let target_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
    let target_role = match target_role {
        Some(r) => r,
        None => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::NotFound));
        }
    };
    if !actor_role.at_least(target_role) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::RoleCap));
    }
    if target_role == WorkspaceRole::Owner && count_owners(&mut tx, workspace_id).await? <= 1 {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastOwner));
    }
    if projects::workspace_removal_blocked_by_private_leads(&mut tx, workspace_id, target_user_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastProjectLead));
    }
    let revoked_invitations = crate::db::invitations::remove_pending_by_inviter(
        &mut tx,
        workspace_id,
        target_user_id,
        &[
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ],
    )
    .await?;
    let transferred_shared_views = crate::db::collections::transfer_shared_view_ownership(
        &mut tx,
        workspace_id,
        target_user_id,
        actor_user_id,
    )
    .await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "role": target_role.as_str(),
        "revokedInvitations": revoked_invitations,
        "transferredSharedViews": transferred_shared_views,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace_member.removed",
            target_type: "workspace_member",
            target_id: target_user_id,
            payload,
            client_ip,
        },
    )
    .await?;
    match tx.commit().await {
        Ok(()) => Ok(Ok(())),
        Err(err) if crate::db::projects::is_private_lead_violation(&err) => {
            Ok(Err(WorkspaceDbError::LastProjectLead))
        }
        Err(err) => Err(err),
    }
}

async fn count_owners(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

const WORKSPACE_PURGE_AFTER_DAYS: i64 = 30;

fn card_count_visible_sql(project_alias: &str) -> String {
    visible_project_sql(project_alias, 2, 3)
}

pub(crate) async fn workspace_card_counts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
) -> Result<(i32, i32), sqlx::Error> {
    let is_guest = role == WorkspaceRole::Guest;
    let visible = card_count_visible_sql("p");
    let document_sql = format!(
        r#"
        SELECT count(*)::bigint
        FROM fvoci.documents d
        WHERE d.workspace_id = $1
          AND d.deleted_at IS NULL
          AND (
            (d.project_id IS NULL AND $2 = false)
            OR (
              d.project_id IS NOT NULL
              AND EXISTS (
                SELECT 1
                FROM fvoci.projects p
                WHERE p.workspace_id = d.workspace_id
                  AND p.id = d.project_id
                  AND p.deleted_at IS NULL
                  AND {visible}
              )
            )
          )
        "#
    );
    let assigned_sql = format!(
        r#"
        SELECT count(*)::bigint
        FROM fvoci.tasks t
        INNER JOIN fvoci.projects p
          ON p.workspace_id = t.workspace_id
         AND p.id = t.project_id
         AND p.deleted_at IS NULL
        WHERE t.workspace_id = $1
          AND t.deleted_at IS NULL
          AND t.archived_at IS NULL
          AND EXISTS (
            SELECT 1 FROM fvoci.task_assignees a
            WHERE a.workspace_id = t.workspace_id
              AND a.task_id = t.id
              AND a.user_id = $3
          )
          AND EXISTS (
            SELECT 1 FROM fvoci.statuses s_open
            WHERE s_open.workspace_id = t.workspace_id
              AND s_open.project_id = t.project_id
              AND s_open.id = t.status_id
              AND s_open.category NOT IN ('done', 'canceled')
          )
          AND {visible}
        "#
    );
    let document_count: i64 = sqlx::query_scalar(&document_sql)
        .bind(workspace_id)
        .bind(is_guest)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
    let assigned_count: i64 = sqlx::query_scalar(&assigned_sql)
        .bind(workspace_id)
        .bind(is_guest)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok((
        i32::try_from(document_count).unwrap_or(i32::MAX),
        i32::try_from(assigned_count).unwrap_or(i32::MAX),
    ))
}

pub async fn trash_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    confirm_slug: &str,
    client_ip: Option<&str>,
) -> Result<Result<(), WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    acquire_admission_lock(&mut tx).await?;
    let member_ids: Vec<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM fvoci.memberships WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_all(&mut *tx)
            .await?;
    let mut lock_ids: Vec<Uuid> = member_ids.into_iter().map(|(id,)| id).collect();
    if !lock_ids.contains(&actor_user_id) {
        lock_ids.push(actor_user_id);
    }
    lock_membership_users(&mut tx, &lock_ids).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if !user_is_active(&mut tx, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !actor_role
        .map(|role| role.at_least(WorkspaceRole::Owner))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let locked = sqlx::query_as::<_, (String, String, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT slug, kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((slug, kind, deleted_at)) = locked else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    if deleted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    if kind == "personal" {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if slug != confirm_slug {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::InvalidInput));
    }
    crate::db::invitations::remove_by_workspace(&mut tx, workspace_id).await?;
    crate::db::api_tokens::remove_by_workspace(&mut tx, workspace_id).await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    let marked: Option<(Uuid,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.workspaces
        SET deleted_at = now(), updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    if marked.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.deleted",
            target_type: "workspace",
            target_id: workspace_id,
            payload: json!({}),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

#[derive(Debug, Clone)]
pub struct WorkspacePurgeResult {
    pub purged: bool,
    pub storage_keys: Vec<String>,
}

impl OperationTx<'_, '_> {
    pub(crate) async fn maintenance_deleted_workspaces(
        &mut self,
        before: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE deleted_at IS NOT NULL AND (kind = 'personal' OR deleted_at <= $1) ORDER BY deleted_at ASC, id ASC")
                .bind(before).fetch_all(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                tx.query("SELECT id FROM workspaces WHERE deleted_at IS NOT NULL AND (kind='personal' OR deleted_at <= ?1) ORDER BY deleted_at, id", &[Cell::instant(before)?])
                    .await?.iter().map(|row| row.cell(0)?.id()).collect()
            }
        }
    }

    /// Keep this actual writer alive across storage cleanup and final deletion.
    /// None means the current deletion/grace predicate no longer authorizes it.
    pub(crate) async fn maintenance_workspace_purge_keys(
        &mut self,
        workspace: Uuid,
        before: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let eligible: Option<bool> = sqlx::query_scalar("SELECT deleted_at IS NOT NULL AND (kind='personal' OR deleted_at <= $2) FROM fvoci.workspaces WHERE id=$1 FOR UPDATE")
                    .bind(workspace).bind(before).fetch_optional(&mut ***tx).await?;
                if eligible != Some(true) {
                    return Ok(None);
                }
                sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE workspace_id=$1 UNION ALL SELECT variants -> 'preview' ->> 'key' FROM fvoci.attachments WHERE workspace_id=$1 AND jsonb_typeof(variants -> 'preview' -> 'key')='string'")
                    .bind(workspace).fetch_all(&mut ***tx).await.map(Some)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace)?;
                let eligible = tx.query("SELECT 1 FROM workspaces WHERE id=?1 AND deleted_at IS NOT NULL AND (kind='personal' OR deleted_at <= ?2)", &[Cell::uuid(workspace), Cell::instant(before)?]).await?;
                if eligible.is_empty() {
                    return Ok(None);
                }
                tx.query("SELECT storage_key FROM attachments WHERE workspace_id=?1 UNION ALL SELECT json_extract(variants,'$.preview.key') FROM attachments WHERE workspace_id=?1 AND json_type(variants,'$.preview.key')='text'", &[Cell::uuid(workspace)])
                    .await?.iter().map(|row| row.cell(0)?.string()).collect::<Result<Vec<_>,_>>().map(Some)
            }
        }
    }

    /// The caller established this current tombstone/grace on the same writer.
    /// Never infer a blanket workspace exclusion for global physical keys.
    pub(crate) async fn maintenance_workspace_purge_attachment_ids(
        &mut self,
        workspace: Uuid,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "family doomed-row reader requires its actual family writer".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_system_context()?;
        tx.require_tenant(workspace)?;
        tx.query(
            "SELECT id FROM attachments WHERE workspace_id=?1 ORDER BY id",
            &[Cell::uuid(workspace)],
        )
        .await?
        .iter()
        .map(|row| row.cell(0)?.id())
        .collect()
    }

    pub(crate) async fn maintenance_purge_workspace(
        &mut self,
        workspace: Uuid,
    ) -> Result<WorkspacePurgeResult, sqlx::Error> {
        let keys: Vec<String>;
        let deleted = match self {
            Self::Postgres(tx) => {
                let eligible: Option<bool> = sqlx::query_scalar(
                    "SELECT deleted_at IS NOT NULL FROM fvoci.workspaces WHERE id=$1 FOR UPDATE",
                )
                .bind(workspace)
                .fetch_optional(&mut ***tx)
                .await?;
                if eligible != Some(true) {
                    return Ok(WorkspacePurgeResult {
                        purged: false,
                        storage_keys: Vec::new(),
                    });
                }
                keys = sqlx::query_scalar(
                    "DELETE FROM fvoci.attachments WHERE workspace_id=$1 RETURNING storage_key",
                )
                .bind(workspace)
                .fetch_all(&mut ***tx)
                .await?;
                sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id=$1")
                    .bind(workspace)
                    .execute(&mut ***tx)
                    .await?;
                sqlx::query("DELETE FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NOT NULL")
                    .bind(workspace)
                    .execute(&mut ***tx)
                    .await?
                    .rows_affected()
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace)?;
                let eligible = tx
                    .query(
                        "SELECT 1 FROM workspaces WHERE id=?1 AND deleted_at IS NOT NULL",
                        &[Cell::uuid(workspace)],
                    )
                    .await?;
                if eligible.is_empty() {
                    return Ok(WorkspacePurgeResult {
                        purged: false,
                        storage_keys: Vec::new(),
                    });
                }
                keys = tx
                    .query(
                        "DELETE FROM attachments WHERE workspace_id=?1 RETURNING storage_key",
                        &[Cell::uuid(workspace)],
                    )
                    .await?
                    .iter()
                    .map(|row| row.cell(0)?.string())
                    .collect::<Result<Vec<_>, _>>()?;
                tx.execute(
                    "DELETE FROM memberships WHERE workspace_id=?1",
                    &[Cell::uuid(workspace)],
                )
                .await?;
                tx.execute(
                    "DELETE FROM workspaces WHERE id=?1 AND deleted_at IS NOT NULL",
                    &[Cell::uuid(workspace)],
                )
                .await?
            }
        };
        if deleted != 1 {
            return Err(sqlx::Error::Protocol(
                "workspaces.purge: locked workspace was not deleted".into(),
            ));
        }
        Ok(WorkspacePurgeResult {
            purged: true,
            storage_keys: keys,
        })
    }
}

pub async fn list_deleted_workspace_ids(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_system(&mut tx).await?;
    let rows = sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT id
        FROM fvoci.workspaces
        WHERE deleted_at IS NOT NULL
          AND (kind = 'personal' OR deleted_at <= $1)
        ORDER BY deleted_at ASC, id ASC
        "#,
    )
    .bind(before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn purge_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<WorkspacePurgeResult, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = crate::db::context::set_system(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    let locked: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((Some(_),)) = locked else {
        crate::db::context::restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(WorkspacePurgeResult {
            purged: false,
            storage_keys: Vec::new(),
        });
    };
    let keys: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM fvoci.attachments WHERE workspace_id = $1 RETURNING storage_key",
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    let deleted: Option<(Uuid,)> = sqlx::query_as(
        "DELETE FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NOT NULL RETURNING id",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    crate::db::context::restore_system(&mut tx, &previous).await?;
    if deleted.is_none() {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol(
            "workspaces.purge: locked workspace was not deleted".into(),
        ));
    }
    tx.commit().await?;
    Ok(WorkspacePurgeResult {
        purged: true,
        storage_keys: keys.into_iter().map(|(key,)| key).collect(),
    })
}

pub async fn sweep_deleted_workspaces(
    pool: &PgPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<WorkspacePurgeResult>, sqlx::Error> {
    let cutoff = now - chrono::Duration::days(WORKSPACE_PURGE_AFTER_DAYS);
    let ids = list_deleted_workspace_ids(pool, cutoff).await?;
    let mut results = Vec::new();
    for id in ids {
        match purge_workspace(pool, id).await {
            Ok(result) => results.push(result),
            Err(err) => {
                tracing::error!(workspace_id = %id, error = %err, "cleanup.workspace_purge_failed");
            }
        }
    }
    Ok(results)
}

async fn fetch_member(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MemberRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, m.role
        FROM fvoci.memberships m
        INNER JOIN fvoci.users u ON u.id = m.user_id
        WHERE m.workspace_id = $1 AND m.user_id = $2 AND u.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(
        row.and_then(|(user_id, email, given_name, family_name, role)| {
            WorkspaceRole::parse(&role).map(|role| MemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        }),
    )
}

#[cfg(feature = "db-tests")]
pub async fn insert_user_for_test(
    pool: &PgPool,
    user_id: Uuid,
    email: &str,
    given_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(email)
    .bind(given_name)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(feature = "db-tests")]
pub async fn add_membership_for_test(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "db-tests")]
pub async fn suspend_user_for_test(pool: &PgPool, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "db-tests")]
pub async fn tenant_context_probe(
    pool: &PgPool,
    tenant_workspace_id: Uuid,
    query_workspace_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, tenant_workspace_id).await?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(query_workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(row.map(|(id,)| id))
}

#[cfg(feature = "db-tests")]
pub async fn lock_sign_in_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    crate::db::identity::lock_sign_in(tx, user_id).await
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod selected_personal_workspace_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use std::future::Future;

    type PublicationRow = (Vec<u8>, Vec<u8>, String, String);

    pub(crate) async fn fixture() -> (Fixture, Uuid) {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        let expiry = crate::db::identity::stored_now()
            + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS);
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4)")
            .bind(credential.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .bind(credential.to_string())
            .bind(expiry.timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        (f, credential)
    }

    pub(crate) async fn snapshot(f: &Fixture) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for sql in [
            "SELECT json_array(hex(id),slug,name,settings,kind,created_at,updated_at,deleted_at,next_document_number,auto_join_domains) FROM workspaces ORDER BY id",
            "SELECT json_array(hex(workspace_id),hex(user_id),role,created_at,updated_at) FROM memberships ORDER BY workspace_id,user_id",
            "SELECT json_array(hex(id),hex(personal_workspace_id),updated_at,deleted_at,suspended_at,anonymized_at,is_instance_admin) FROM users ORDER BY id",
            "SELECT json_array(hex(id),seq,hex(workspace_id),hex(actor_user_id),verb,target_type,hex(target_id),payload,channel,created_at) FROM events ORDER BY id",
            "SELECT json_array(hex(id),hex(workspace_id),hex(actor_user_id),verb,target_type,hex(target_id),payload,ip,created_at) FROM audit_log ORDER BY id",
            "SELECT json_array(id,last_seq) FROM event_sequence ORDER BY id",
        ] { rows.push(sqlx::query_scalar(sql).fetch_all(&f.pool).await.unwrap()); }
        rows
    }

    pub(crate) async fn foreign_keys(f: &Fixture) {
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&f.pool)
            .await
            .unwrap()
            .is_empty());
    }

    async fn ensure(
        f: &Fixture,
        credential: Uuid,
    ) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
        ensure_personal_workspace_backend(
            &f.backend,
            &crate::license::absent(),
            f.user,
            credential,
            Some("203.0.113.70"),
        )
        .await
    }

    pub(crate) async fn assert_publication(f: &Fixture, meta: &WorkspaceMeta, ip: Option<&str>) {
        assert!(!meta.id.is_nil());
        assert_ne!(meta.id, f.workspace);
        assert_eq!(meta.name, "Personal");
        assert_eq!(
            crate::validate::normalize_slug(&meta.slug).unwrap(),
            meta.slug
        );
        let mapping: Vec<u8> =
            sqlx::query_scalar("SELECT personal_workspace_id FROM users WHERE id=?1")
                .bind(f.user.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(mapping, meta.id.as_bytes());
        let stored: (String, String, String) =
            sqlx::query_as("SELECT kind,name,slug FROM workspaces WHERE id=?1")
                .bind(meta.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            stored,
            ("personal".into(), "Personal".into(), meta.slug.clone())
        );
        let owners: Vec<(Vec<u8>, String)> =
            sqlx::query_as("SELECT user_id,role FROM memberships WHERE workspace_id=?1")
                .bind(meta.id.as_bytes().as_slice())
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(owners, vec![(f.user.as_bytes().to_vec(), "owner".into())]);
        let payload = json!({"workspaceId":meta.id.to_string(),"ownerId":f.user.to_string(),"kind":"personal","slug":meta.slug});
        for table in ["events", "audit_log"] {
            let rows:Vec<PublicationRow> = sqlx::query_as(&format!("SELECT workspace_id,actor_user_id,target_type,payload FROM {table} WHERE verb='workspace.personal_created' AND target_id=?1"))
                .bind(meta.id.as_bytes().as_slice()).fetch_all(&f.pool).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, meta.id.as_bytes());
            assert_eq!(rows[0].1, f.user.as_bytes());
            assert_eq!(rows[0].2, "workspace");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&rows[0].3).unwrap(),
                payload
            );
        }
        let actual: Option<String> = sqlx::query_scalar(
            "SELECT ip FROM audit_log WHERE verb='workspace.personal_created' AND target_id=?1",
        )
        .bind(meta.id.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(actual.as_deref(), ip);
    }

    #[tokio::test]
    async fn sqlite_personal_bootstrap_stable_concurrent_mapping_and_private_slug_collision() {
        let (f, credential) = fixture().await;
        let canonical = personal_workspace_slug(f.user);
        sqlx::query("UPDATE workspaces SET slug=?1,name='Supported ordinary team' WHERE id=?2")
            .bind(&canonical)
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let team: String = sqlx::query_scalar(
            "SELECT json_array(hex(id),slug,name,kind,settings) FROM workspaces WHERE id=?1",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let before_create = snapshot(&f).await;
        let (a, b) = tokio::join!(ensure(&f, credential), ensure(&f, credential));
        let a = a.unwrap().unwrap();
        let b = b.unwrap().unwrap();
        assert_eq!((a.id, &a.name, &a.slug), (b.id, &b.name, &b.slug));
        assert_ne!(a.slug, canonical);
        assert_publication(&f, &a, Some("203.0.113.70")).await;
        let before = snapshot(&f).await;
        for index in [0, 1, 3, 4] {
            assert_eq!(before[index].len(), before_create[index].len() + 1);
        }
        assert_eq!(before[2].len(), before_create[2].len());
        let replay = ensure(&f, credential).await.unwrap().unwrap();
        assert_eq!(
            (replay.id, replay.name, replay.slug),
            (a.id, a.name, a.slug)
        );
        assert_eq!(snapshot(&f).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT json_array(hex(id),slug,name,kind,settings) FROM workspaces WHERE id=?1"
            )
            .bind(f.workspace.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            team
        );
        foreign_keys(&f).await;
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_personal_bootstrap_current_credentials_mapping_and_queued_writer_denials() {
        let (f, credential) = fixture().await;
        // A wrong-kind private mapping is replaced, never converted or returned.
        sqlx::query("UPDATE users SET personal_workspace_id=?1 WHERE id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(matches!(
            ensure_personal_workspace_backend(
                &f.backend,
                &crate::license::absent(),
                Uuid::now_v7(),
                credential,
                None
            )
            .await
            .unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        assert_eq!(snapshot(&f).await, before);
        let meta = ensure(&f, credential).await.unwrap().unwrap();
        assert_eq!(meta.slug, personal_workspace_slug(f.user));
        assert_publication(&f, &meta, Some("203.0.113.70")).await;
        for (deny, restore, id) in [
            (
                "UPDATE users SET suspended_at=1 WHERE id=?1",
                "UPDATE users SET suspended_at=NULL WHERE id=?1",
                f.user,
            ),
            (
                "UPDATE users SET deleted_at=1 WHERE id=?1",
                "UPDATE users SET deleted_at=NULL WHERE id=?1",
                f.user,
            ),
            (
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                "UPDATE sessions SET revoked_at=NULL WHERE id=?1",
                credential,
            ),
        ] {
            sqlx::query(deny)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let before = snapshot(&f).await;
            assert!(matches!(
                ensure(&f, credential).await.unwrap(),
                Err(WorkspaceDbError::Forbidden)
            ));
            assert_eq!(snapshot(&f).await, before);
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(ensure(&f, credential).await.unwrap().unwrap().id, meta.id);
        }
        let before = snapshot(&f).await;
        let mut blocker = f.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        let mut pending = Box::pin(ensure(&f, credential));
        std::future::poll_fn(|cx| {
            assert!(
                pending.as_mut().poll(cx).is_pending(),
                "must await the held one-connection writer"
            );
            std::task::Poll::Ready(())
        })
        .await;
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&mut *blocker)
            .await
            .unwrap();
        blocker.commit().await.unwrap();
        assert!(matches!(
            pending.await.unwrap(),
            Err(WorkspaceDbError::Forbidden)
        ));
        assert_eq!(snapshot(&f).await, before);
        let expiry = crate::db::identity::stored_now()
            + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS);
        sqlx::query("UPDATE sessions SET expires_at=?1 WHERE id=?2")
            .bind(expiry.timestamp_micros())
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // A deleted mapped workspace is not resurrected; its slug stays reserved.
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(meta.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let fresh = ensure(&f, credential).await.unwrap().unwrap();
        assert_ne!(fresh.id, meta.id);
        assert_ne!(fresh.slug, meta.slug);
        assert_publication(&f, &fresh, Some("203.0.113.70")).await;
        foreign_keys(&f).await;
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_personal_bootstrap_event_audit_and_commit_fk_failures_then_healthy_retry() {
        let (f, credential) = fixture().await;
        let before = snapshot(&f).await;
        for table in ["events", "audit_log"] {
            sqlx::query(&format!("CREATE TRIGGER personal_refuse BEFORE INSERT ON {table} WHEN NEW.verb='workspace.personal_created' BEGIN SELECT RAISE(ABORT,'personal publication refused'); END"))
                .execute(&f.pool).await.unwrap();
            let error = ensure(&f, credential).await.err().unwrap();
            assert!(
                matches!(&error,sqlx::Error::Database(e) if e.message().contains("personal publication refused"))
            );
            assert_eq!(snapshot(&f).await, before);
            sqlx::query("DROP TRIGGER personal_refuse")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("CREATE TABLE personal_commit_fk_probe(workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED) STRICT")
            .execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER personal_commit_refuse AFTER INSERT ON audit_log WHEN NEW.verb='workspace.personal_created' BEGIN INSERT INTO personal_commit_fk_probe VALUES(zeroblob(16)); END")
            .execute(&f.pool).await.unwrap();
        let error = ensure(&f, credential).await.err().unwrap();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("typed original commit receipt required")
        };
        let receipt = source
            .downcast_ref::<super::super::backend::CommitCleanupUnknown>()
            .unwrap();
        assert_eq!(
            receipt.settlement,
            super::super::backend::CommitSettlement::LocalWriterReconcile
        );
        assert!(
            matches!(&receipt.source.source,sqlx::Error::Database(e) if e.message().contains("FOREIGN KEY"))
        );
        assert_eq!(snapshot(&f).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM personal_commit_fk_probe")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER personal_commit_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = ensure(&f, credential).await.unwrap().unwrap();
        assert_publication(&f, &healthy, Some("203.0.113.70")).await;
        foreign_keys(&f).await;
        f.close().await;
    }

    #[test]
    fn personal_bootstrap_rollback_cleanup_retains_domain_and_driver_causes() {
        for (index, result) in [
            Ok(Err(WorkspaceDbError::Forbidden)),
            Err(sqlx::Error::Protocol("original bootstrap driver".into())),
        ]
        .into_iter()
        .enumerate()
        {
            // Returned-error propagation only, not an actual provider rollback failure.
            let error = personal_workspace_after_rollback(
                result,
                Err(sqlx::Error::Protocol("synthetic cleanup failure".into())),
            )
            .err()
            .unwrap();
            assert!(super::super::backend::is_rollback_cleanup_unknown(&error));
            let sqlx::Error::AnyDriverError(source) = error else {
                panic!("typed canonical receipt required")
            };
            let receipt = source
                .downcast_ref::<super::super::backend::RollbackCleanupUnknown>()
                .unwrap();
            let original = receipt.original.as_ref().unwrap();
            if index == 0 {
                assert!(original
                    .downcast_ref::<PersonalWorkspaceRefusal>()
                    .is_some_and(|r| matches!(&r.0, WorkspaceDbError::Forbidden)));
            } else {
                assert!(original.downcast_ref::<sqlx::Error>().is_some_and(
                    |e| matches!(e,sqlx::Error::Protocol(s) if s=="original bootstrap driver")
                ));
            }
            assert!(
                matches!(&receipt.cleanup,sqlx::Error::Protocol(s) if s=="synthetic cleanup failure")
            );
        }
    }
}
