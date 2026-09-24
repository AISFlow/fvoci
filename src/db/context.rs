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

pub const CREDENTIAL_LIVE_SQL: &str = r#"
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
