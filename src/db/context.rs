use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub const MEMBERSHIP_LOCK_NAMESPACE: i32 = 1_907_006;
pub const TREE_LOCK_NAMESPACE: i32 = 1_907_005;

pub async fn set_tenant(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn set_system(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
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

pub async fn recheck_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (
            s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
            AND u.deleted_at IS NULL
            AND u.suspended_at IS NULL
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        FOR UPDATE OF u, s
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

pub async fn session_is_live(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (
            s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
            AND u.deleted_at IS NULL
            AND u.suspended_at IS NULL
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        "#,
    )
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
