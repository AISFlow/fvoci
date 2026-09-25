use std::collections::HashSet;

use chrono::NaiveDate;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{membership_role, membership_role_for_update, workspace_is_live};
use crate::db::workspace::WorkspaceRole;

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

pub async fn remove_workspace_holiday(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    on_date: NaiveDate,
) -> Result<Result<(), HolidayDbError>, sqlx::Error> {
    mutate_holiday(pool, workspace_id, actor_user_id, session_id, on_date, true).await
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
