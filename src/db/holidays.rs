use std::collections::HashSet;

use chrono::NaiveDate;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, DbTx};
use super::codec::Cell;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::workspace::{
    membership_role, membership_role_for_update, workspace_is_live, WorkspaceRole,
};

#[derive(Debug)]
pub enum HolidayDbError {
    NotFound,
    Forbidden,
}

#[derive(Debug, Clone)]
pub struct HolidayList {
    pub items: Vec<NaiveDate>,
    pub can_edit: bool,
}

pub async fn list_holiday_dates(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<HashSet<NaiveDate>, sqlx::Error> {
    let rows: Vec<(NaiveDate,)> = sqlx::query_as(
        r#"
        SELECT date
        FROM fvoci.workspace_holidays
        WHERE workspace_id = $1
        ORDER BY date
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(date,)| date).collect())
}

pub async fn list_workspace_holidays(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<HolidayList, HolidayDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::NotFound));
    }
    let Some(role) = membership_role(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::NotFound));
    };
    let items: Vec<NaiveDate> = sqlx::query_scalar(
        r#"
        SELECT date
        FROM fvoci.workspace_holidays
        WHERE workspace_id = $1
        ORDER BY date
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(HolidayList {
        items,
        can_edit: role.at_least(WorkspaceRole::Admin),
    }))
}

pub async fn list_workspace_holidays_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<HolidayList, HolidayDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_workspace_holidays(pool, workspace, actor, credential).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace).await?;
    let role = match holiday_access(&mut tx, workspace, actor, credential, false).await? {
        Ok(role) => role,
        Err(error) => {
            tx.rollback().await?;
            return Ok(Err(error));
        }
    };
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let items = family
        .query(
            "SELECT date FROM workspace_holidays WHERE workspace_id=?1 ORDER BY date",
            &[Cell::uuid(workspace)],
        )
        .await?
        .iter()
        .map(|r| r.cell(0)?.date())
        .collect::<Result<_, _>>()?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(HolidayList {
        items,
        can_edit: role.at_least(WorkspaceRole::Admin),
    }))
}

async fn holiday_access(
    tx: &mut DbTx,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    write: bool,
) -> Result<Result<WorkspaceRole, HolidayDbError>, sqlx::Error> {
    let mut op = tx.operation();
    if write {
        op.lock_membership_users(&[actor]).await?;
    }
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(HolidayDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await? {
        return Ok(Err(HolidayDbError::NotFound));
    }
    let Some(role) = op.membership_role(workspace, actor, write).await? else {
        return Ok(Err(HolidayDbError::NotFound));
    };
    if write && !role.at_least(WorkspaceRole::Admin) {
        return Ok(Err(HolidayDbError::NotFound));
    }
    Ok(Ok(role))
}

pub async fn add_workspace_holiday(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    on_date: NaiveDate,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    mutate_holiday(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        on_date,
        false,
    )
    .await
}

pub async fn add_workspace_holiday_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    date: NaiveDate,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return add_workspace_holiday(pool, workspace, actor, credential, date).await;
    }
    mutate_holiday_backend(backend, workspace, actor, credential, date, false).await
}

pub async fn remove_workspace_holiday(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    on_date: NaiveDate,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    mutate_holiday(pool, workspace_id, actor_user_id, session_id, on_date, true).await
}

pub async fn remove_workspace_holiday_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    date: NaiveDate,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return remove_workspace_holiday(pool, workspace, actor, credential, date).await;
    }
    mutate_holiday_backend(backend, workspace, actor, credential, date, true).await
}

async fn mutate_holiday_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    date: NaiveDate,
    remove: bool,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if let Err(error) = holiday_access(&mut tx, workspace, actor, credential, true).await? {
        tx.rollback().await?;
        return Ok(Err(error));
    }
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    family.require_writer()?;
    family.require_tenant(workspace)?;
    family.execute(if remove {"DELETE FROM workspace_holidays WHERE workspace_id=?1 AND date=?2"} else {"INSERT INTO workspace_holidays(workspace_id,date) VALUES(?1,?2) ON CONFLICT DO NOTHING"}, &[Cell::uuid(workspace),Cell::text(date.format("%Y-%m-%d").to_string())]).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    Ok(Ok(()))
}

async fn mutate_holiday(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    on_date: NaiveDate,
    remove: bool,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::NotFound));
    }
    let Some(role) = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::NotFound));
    };
    if !role.at_least(WorkspaceRole::Admin) {
        tx.rollback().await?;
        return Ok(Err(HolidayDbError::NotFound));
    }
    if remove {
        sqlx::query(
            r#"
            DELETE FROM fvoci.workspace_holidays
            WHERE workspace_id = $1 AND date = $2
            "#,
        )
        .bind(workspace_id)
        .bind(on_date)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO fvoci.workspace_holidays (workspace_id, date)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(workspace_id)
        .bind(on_date)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(()))
}
