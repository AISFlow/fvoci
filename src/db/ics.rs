use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, DbTx};
use super::codec::Cell;

use crate::auth::token::{hash_token, new_token, token_hashes_eq};
use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, set_system, set_tenant,
};
use crate::db::projects::visible_project_sql;
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::ics::{ics_etag, render_ics, IcsTask};

const ICS_TOKEN_TTL_SECS: i64 = 365 * 24 * 60 * 60;
const ICS_FEED_LIMIT: i64 = 500;

type IcsTokenLookupRow = (Uuid, Uuid, String, Option<DateTime<Utc>>, DateTime<Utc>);
type IcsTaskRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    Option<NaiveDate>,
    Option<NaiveDate>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

#[derive(Debug)]
pub enum IcsDbError {
    NotFound,
    Forbidden,
}

#[derive(Debug, Clone)]
pub struct IcsFeed {
    pub ics: String,
    pub etag: String,
}

pub async fn rotate_ics_token(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    public_origin: &str,
) -> Result<Result<String, IcsDbError>, sqlx::Error> {
    let issued = new_token();
    let expires_at = Utc::now() + chrono::Duration::seconds(ICS_TOKEN_TTL_SECS);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(IcsDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(IcsDbError::NotFound));
    }
    if membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .is_none()
    {
        tx.rollback().await?;
        return Ok(Err(IcsDbError::NotFound));
    }
    let existing: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.ics_tokens
        WHERE workspace_id = $1 AND user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let id = existing.map(|(id,)| id).unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        r#"
        INSERT INTO fvoci.ics_tokens (
            id, workspace_id, user_id, token_hash, expires_at, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, now(), now())
        ON CONFLICT (workspace_id, user_id) DO UPDATE
        SET token_hash = EXCLUDED.token_hash,
            expires_at = EXCLUDED.expires_at,
            updated_at = now()
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(&issued.hash)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(format!(
        "{}/api/v1/ics/{}",
        public_origin.trim_end_matches('/'),
        issued.token
    )))
}

pub async fn rotate_ics_token_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    public_origin: &str,
) -> Result<Result<String, IcsDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return rotate_ics_token(pool, workspace, actor, credential, public_origin).await;
    }
    let issued = new_token();
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    tx.operation().lock_membership_users(&[actor]).await?;
    if !tx.operation().recheck_session(actor, credential).await? {
        tx.rollback().await?;
        return Ok(Err(IcsDbError::Forbidden));
    }
    if !tx.operation().workspace_is_live(workspace).await?
        || tx
            .operation()
            .membership_role(workspace, actor, true)
            .await?
            .is_none()
    {
        tx.rollback().await?;
        return Ok(Err(IcsDbError::NotFound));
    }
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    family.require_writer()?;
    family.require_tenant(workspace)?;
    family.execute("INSERT INTO ics_tokens(id,workspace_id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4,(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)+?5) ON CONFLICT(workspace_id,user_id) DO UPDATE SET token_hash=excluded.token_hash,expires_at=excluded.expires_at,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000", &[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(actor),Cell::text(issued.hash),Cell::Integer(ICS_TOKEN_TTL_SECS*1_000_000)]).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(format!(
        "{}/api/v1/ics/{}",
        public_origin.trim_end_matches('/'),
        issued.token
    )))
}

pub async fn read_ics_by_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<IcsFeed>, sqlx::Error> {
    let computed_hash = hash_token(raw_token);
    let mut lookup = pool.begin().await?;
    let previous = set_system(&mut lookup).await?;
    let row: Option<IcsTokenLookupRow> = sqlx::query_as(
        r#"
        SELECT workspace_id, user_id, token_hash, expires_at, updated_at
        FROM fvoci.ics_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(&computed_hash)
    .fetch_optional(&mut *lookup)
    .await?;
    restore_system(&mut lookup, &previous).await?;
    lookup.commit().await?;
    let Some((workspace_id, user_id, stored_hash, expires_at, token_updated_at)) = row else {
        return Ok(None);
    };
    if !token_hashes_eq(&stored_hash, &computed_hash) {
        return Ok(None);
    }
    if expires_at.is_some_and(|at| at <= Utc::now()) {
        return Ok(None);
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let user: Option<(Option<DateTime<Utc>>, String)> = sqlx::query_as(
        r#"
        SELECT suspended_at, timezone
        FROM fvoci.users
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((suspended_at, timezone)) = user else {
        tx.rollback().await?;
        return Ok(None);
    };
    if suspended_at.is_some() {
        tx.rollback().await?;
        return Ok(None);
    }
    let Some(role) = membership_role(&mut tx, workspace_id, user_id).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    let is_guest = role == WorkspaceRole::Guest;
    let tasks = list_visible_ics_tasks(&mut tx, workspace_id, user_id, is_guest).await?;
    tx.commit().await?;

    let time_zone = if timezone.is_empty() {
        "Asia/Seoul"
    } else {
        timezone.as_str()
    };
    let rendered = tasks
        .iter()
        .map(
            |(id, _workspace_id, _project_id, title, start_date, due_date, due_at, updated_at)| {
                IcsTask {
                    id: id.to_string(),
                    title: title.clone(),
                    start_date: *start_date,
                    due_date: *due_date,
                    due_at: *due_at,
                    updated_at: *updated_at,
                }
            },
        )
        .collect::<Vec<_>>();
    let ics = render_ics(&rendered, time_zone, Utc::now());
    let etag = ics_etag(token_updated_at, &rendered);
    Ok(Some(IcsFeed { ics, etag }))
}

pub async fn read_ics_by_token_backend(
    backend: &Backend,
    raw_token: &str,
) -> Result<Option<IcsFeed>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return read_ics_by_token(pool, raw_token).await;
    }
    let hash = hash_token(raw_token);
    // Token lookup and current membership/task projection share one snapshot.
    let mut tx = backend.begin_read().await?;
    let previous = tx.operation().set_system().await?;
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    family.require_system_context()?;
    let rows = family.query("SELECT workspace_id,user_id,token_hash,updated_at FROM ics_tokens WHERE token_hash=?1 AND (expires_at IS NULL OR expires_at>unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)", &[Cell::text(&hash)]).await?;
    let Some(row) = rows.first() else {
        tx.rollback().await?;
        return Ok(None);
    };
    let workspace = row.cell(0)?.id()?;
    let actor = row.cell(1)?.id()?;
    if !token_hashes_eq(&row.cell(2)?.string()?, &hash) {
        tx.rollback().await?;
        return Ok(None);
    }
    let updated = row.cell(3)?.datetime()?;
    tx.operation().restore_system(previous).await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx.operation().workspace_is_live(workspace).await?
        || tx
            .operation()
            .membership_role(workspace, actor, false)
            .await?
            .is_none()
    {
        tx.rollback().await?;
        return Ok(None);
    }
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    family.require_tenant(workspace)?;
    let users=family.query("SELECT timezone FROM users WHERE id=?1 AND deleted_at IS NULL AND suspended_at IS NULL", &[Cell::uuid(actor)]).await?;
    let Some(user) = users.first() else {
        tx.rollback().await?;
        return Ok(None);
    };
    let timezone = user.cell(0)?.string()?;
    let rows=family.query("SELECT t.id,t.project_id,t.title,t.start_date,t.due_date,t.due_at,t.updated_at FROM tasks t JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id WHERE t.workspace_id=?1 AND t.archived_at IS NULL AND t.deleted_at IS NULL AND p.deleted_at IS NULL AND (t.start_date IS NOT NULL OR t.due_date IS NOT NULL OR t.due_at IS NOT NULL) AND (EXISTS(SELECT 1 FROM task_assignees a WHERE a.workspace_id=t.workspace_id AND a.task_id=t.id AND a.user_id=?2) OR EXISTS(SELECT 1 FROM views v WHERE v.workspace_id=t.workspace_id AND v.user_id=?2 AND v.type='calendar' AND v.project_id=t.project_id)) ORDER BY COALESCE(t.due_date,date(t.due_at/1000000,'unixepoch'),t.start_date),t.id", &[Cell::uuid(workspace),Cell::uuid(actor)]).await?;
    let mut tasks = Vec::new();
    for row in rows {
        if !tx
            .operation()
            .project_permission_by_id(workspace, actor, row.cell(1)?.id()?)
            .await?
            .is_some_and(|p| p.at_least(crate::projects::ProjectPermission::View))
        {
            continue;
        }
        tasks.push(IcsTask {
            id: row.cell(0)?.id()?.to_string(),
            title: row.cell(2)?.string()?,
            start_date: row.cell(3)?.optional(Cell::date)?,
            due_date: row.cell(4)?.optional(Cell::date)?,
            due_at: row.cell(5)?.optional(Cell::datetime)?,
            updated_at: row.cell(6)?.datetime()?,
        });
        if tasks.len() == ICS_FEED_LIMIT as usize {
            break;
        }
    }
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let now = super::tasks::family_clock(family).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Some(IcsFeed {
        ics: render_ics(
            &tasks,
            if timezone.is_empty() {
                "Asia/Seoul"
            } else {
                &timezone
            },
            now,
        ),
        etag: ics_etag(updated, &tasks),
    }))
}

async fn list_visible_ics_tasks(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    is_guest: bool,
) -> Result<Vec<IcsTaskRow>, sqlx::Error> {
    let visible = visible_project_sql("p", 2, 3);
    let sql = format!(
        r#"
        SELECT t.id, t.workspace_id, t.project_id, t.title,
               t.start_date, t.due_date, t.due_at, t.updated_at
        FROM fvoci.tasks t
        INNER JOIN fvoci.projects p
            ON p.workspace_id = t.workspace_id AND p.id = t.project_id
        WHERE t.workspace_id = $1
          AND t.archived_at IS NULL
          AND t.deleted_at IS NULL
          AND p.deleted_at IS NULL
          AND (
                t.start_date IS NOT NULL
                OR t.due_date IS NOT NULL
                OR t.due_at IS NOT NULL
          )
          AND {visible}
          -- Source listIcsFeed: the owner's assigned tasks, plus every dated task
          -- of a project where the owner keeps a saved calendar view.
          AND (
                EXISTS (
                    SELECT 1 FROM fvoci.task_assignees ta
                    WHERE ta.workspace_id = t.workspace_id AND ta.task_id = t.id AND ta.user_id = $3
                )
                OR EXISTS (
                    SELECT 1 FROM fvoci.views v
                    WHERE v.workspace_id = t.workspace_id AND v.user_id = $3
                      AND v.type = 'calendar' AND v.project_id = t.project_id
                )
          )
        ORDER BY COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date, t.start_date), t.id
        LIMIT {ICS_FEED_LIMIT}
        "#
    );
    sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(is_guest)
        .bind(user_id)
        .fetch_all(&mut **tx)
        .await
}
