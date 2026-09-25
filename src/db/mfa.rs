//! TOTP MFA storage and the single sign-in gate.
//!
//! Source `packages/core/src/mfa.ts`. Every first factor (password, magic
//! link, invitation, OIDC) ends in [`issue_session_or_challenge`]: an account
//! with MFA enabled gets a 5-minute challenge token instead of a session, and
//! only [`complete_challenge`] turns it into one.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token::{new_token, SESSION_TTL_SECS};
use crate::db::account::{record_account_change, AccountRecord};
use crate::db::context::{clear_self_user, recheck_session, set_self_user};
use crate::db::identity::{append_event, create_session, lock_sign_in, EventAppend};

/// Source `MFA_PENDING_TTL_SECONDS`.
pub const MFA_CHALLENGE_TTL_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct MfaRow {
    pub user_id: Uuid,
    pub totp_secret: String,
    pub enabled_at: Option<DateTime<Utc>>,
    pub recovery_hashes: Vec<String>,
    pub last_used_step: Option<i32>,
}

/// Outcome of a first factor.
#[derive(Debug)]
pub enum Issued {
    Session { user_id: Uuid, token: String },
    Challenge { mfa_token: String },
}

#[derive(Debug, Default, Clone, Copy)]
pub struct IssueOptions {
    /// Magic link: the token's generation must still be current.
    pub generation: Option<i32>,
    /// Magic link proves the mailbox.
    pub mark_email_verified: bool,
}

type MfaTuple = (
    Uuid,
    String,
    Option<DateTime<Utc>>,
    Vec<String>,
    Option<i32>,
);

fn row_from(t: MfaTuple) -> MfaRow {
    MfaRow {
        user_id: t.0,
        totp_secret: t.1,
        enabled_at: t.2,
        recovery_hashes: t.3,
        last_used_step: t.4,
    }
}

const SELECT_MFA: &str = r#"
    SELECT user_id, totp_secret, enabled_at, recovery_hashes, last_used_step
    FROM fvoci.user_mfa
    WHERE user_id = $1
"#;

/// Reads the row under the owner's RLS context (restores it afterwards).
pub(crate) async fn find_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<MfaRow>, sqlx::Error> {
    set_self_user(tx, user_id).await?;
    let row: Option<MfaTuple> = sqlx::query_as(SELECT_MFA)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?;
    clear_self_user(tx).await?;
    Ok(row.map(row_from))
}

pub async fn find(pool: &PgPool, user_id: Uuid) -> Result<Option<MfaRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = find_in_tx(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(row)
}

/// New secret and recovery hashes on a not-yet-enabled row. False when MFA is
/// already enabled (the caller must disable first).
pub async fn setup(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    sealed_secret: &str,
    recovery_hashes: &[String],
) -> Result<SetupOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(SetupOutcome::SessionGone);
    }
    set_self_user(&mut tx, user_id).await?;
    let stored: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO fvoci.user_mfa (user_id, totp_secret, recovery_hashes)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id) DO UPDATE
        SET totp_secret = EXCLUDED.totp_secret,
            recovery_hashes = EXCLUDED.recovery_hashes,
            last_used_step = NULL,
            updated_at = now()
        WHERE fvoci.user_mfa.enabled_at IS NULL
        RETURNING user_id
        "#,
    )
    .bind(user_id)
    .bind(sealed_secret)
    .bind(recovery_hashes)
    .fetch_optional(&mut *tx)
    .await?;
    clear_self_user(&mut tx).await?;
    if stored.is_none() {
        tx.rollback().await?;
        return Ok(SetupOutcome::AlreadyEnabled);
    }
    tx.commit().await?;
    Ok(SetupOutcome::Stored)
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Stored,
    AlreadyEnabled,
    SessionGone,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnableOutcome {
    Ok,
    NotSetup,
    SessionGone,
}

/// Source `enableMfa` transaction: the verified step becomes the replay floor.
pub async fn enable(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    expected_secret: &str,
    step: i64,
    ip: Option<&str>,
) -> Result<EnableOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(EnableOutcome::SessionGone);
    }
    set_self_user(&mut tx, user_id).await?;
    // The secret must still be the one the code was checked against: a
    // concurrent setup replaced it otherwise.
    let enabled = sqlx::query(
        r#"
        UPDATE fvoci.user_mfa
        SET enabled_at = now(), last_used_step = $2, updated_at = now()
        WHERE user_id = $1 AND enabled_at IS NULL AND totp_secret = $3
        "#,
    )
    .bind(user_id)
    .bind(step as i32)
    .bind(expected_secret)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    clear_self_user(&mut tx).await?;
    if !enabled {
        tx.rollback().await?;
        return Ok(EnableOutcome::NotSetup);
    }
    record_account_change(&mut tx, mfa_record("auth.mfa_enabled", user_id, ip)).await?;
    tx.commit().await?;
    Ok(EnableOutcome::Ok)
}

fn mfa_record<'a>(verb: &'a str, user_id: Uuid, ip: Option<&'a str>) -> AccountRecord<'a> {
    AccountRecord {
        verb,
        actor_user_id: Some(user_id),
        target_id: user_id,
        payload: json!({ "userId": user_id.to_string() }),
        audit_payload: None,
        ip,
        channel: "web",
        workspace_id: None,
        target_type: "user",
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DisableOutcome {
    Ok,
    NotEnabled,
    SessionGone,
}

pub async fn disable(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    ip: Option<&str>,
) -> Result<DisableOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(DisableOutcome::SessionGone);
    }
    set_self_user(&mut tx, user_id).await?;
    let removed =
        sqlx::query("DELETE FROM fvoci.user_mfa WHERE user_id = $1 AND enabled_at IS NOT NULL")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
    clear_self_user(&mut tx).await?;
    if !removed {
        tx.rollback().await?;
        return Ok(DisableOutcome::NotEnabled);
    }
    record_account_change(&mut tx, mfa_record("auth.mfa_disabled", user_id, ip)).await?;
    tx.commit().await?;
    Ok(DisableOutcome::Ok)
}

/// Source `claimStep`: only a step newer than the last accepted one.
pub(crate) async fn claim_step(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    step: i64,
) -> Result<bool, sqlx::Error> {
    set_self_user(tx, user_id).await?;
    let claimed = sqlx::query(
        r#"
        UPDATE fvoci.user_mfa
        SET last_used_step = $2, updated_at = now()
        WHERE user_id = $1
          AND enabled_at IS NOT NULL
          AND (last_used_step IS NULL OR last_used_step < $2)
        "#,
    )
    .bind(user_id)
    .bind(step as i32)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    clear_self_user(tx).await?;
    Ok(claimed)
}

/// Source `consumeRecovery`: one use per code.
pub(crate) async fn consume_recovery(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    hash: &str,
) -> Result<bool, sqlx::Error> {
    set_self_user(tx, user_id).await?;
    let consumed = sqlx::query(
        r#"
        UPDATE fvoci.user_mfa
        SET recovery_hashes = array_remove(recovery_hashes, $2), updated_at = now()
        WHERE user_id = $1
          AND enabled_at IS NOT NULL
          AND $2 = ANY (recovery_hashes)
        "#,
    )
    .bind(user_id)
    .bind(hash)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        == 1;
    clear_self_user(tx).await?;
    Ok(consumed)
}

async fn live_generation(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<i32>, sqlx::Error> {
    let row: Option<(i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT auth_generation, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((generation, None)) => Some(generation),
        _ => None,
    })
}

/// Session row + `auth.login` event in the caller's transaction.
pub(crate) async fn issue_session_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    method: &str,
) -> Result<String, sqlx::Error> {
    let token = new_token();
    let expires_at = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
    create_session(tx, Uuid::now_v7(), user_id, &token.hash, expires_at).await?;
    let previous = crate::db::context::set_system(tx).await?;
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(user_id),
            verb: "auth.login".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(user_id),
            payload: json!({ "userId": user_id.to_string(), "method": method }),
        },
    )
    .await?;
    crate::db::context::restore_system(tx, &previous).await?;
    Ok(token.token)
}

/// Source `issueSessionOrChallenge`. None when the account is gone,
/// suspended, or (magic link) its generation moved on.
pub async fn issue_session_or_challenge(
    pool: &PgPool,
    user_id: Uuid,
    method: &str,
    opts: IssueOptions,
) -> Result<Option<Issued>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, user_id).await?;
    let Some(generation) = live_generation(&mut tx, user_id).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    if opts
        .generation
        .is_some_and(|expected| expected != generation)
    {
        tx.rollback().await?;
        return Ok(None);
    }
    if opts.mark_email_verified {
        let verified: bool = sqlx::query_scalar("SELECT fvoci.app_user_mark_email_verified($1)")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        if !verified {
            tx.rollback().await?;
            return Ok(None);
        }
    }
    let mfa = find_in_tx(&mut tx, user_id).await?;
    if mfa.is_none_or(|row| row.enabled_at.is_none()) {
        let token = issue_session_in_tx(&mut tx, user_id, method).await?;
        tx.commit().await?;
        return Ok(Some(Issued::Session { user_id, token }));
    }
    let challenge = new_token();
    sqlx::query("SELECT fvoci.app_mfa_challenge_issue($1, $2, $3, $4)")
        .bind(&challenge.hash)
        .bind(user_id)
        .bind(generation)
        .bind(Utc::now() + Duration::seconds(MFA_CHALLENGE_TTL_SECS))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(Issued::Challenge {
        mfa_token: challenge.token,
    }))
}

/// The pending challenge's owner, without consuming it.
pub async fn peek_challenge(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<(Uuid, i32)>, sqlx::Error> {
    sqlx::query_as("SELECT user_id, generation FROM fvoci.app_mfa_challenge_peek($1)")
        .bind(token_hash)
        .fetch_optional(pool)
        .await
}

/// How the second factor matched, recorded as the login method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondFactor {
    Totp(i64),
    Recovery,
}

impl SecondFactor {
    pub fn method(self) -> &'static str {
        match self {
            Self::Totp(_) => "totp",
            Self::Recovery => "recovery",
        }
    }
}

/// Claims the verified factor (TOTP step or recovery hash) in `tx`.
pub(crate) async fn claim_factor(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    factor: SecondFactor,
    recovery_hash: Option<&str>,
) -> Result<bool, sqlx::Error> {
    match (factor, recovery_hash) {
        (SecondFactor::Totp(step), _) => claim_step(tx, user_id, step).await,
        (SecondFactor::Recovery, Some(hash)) => consume_recovery(tx, user_id, hash).await,
        (SecondFactor::Recovery, None) => Ok(false),
    }
}

/// Claims a factor outside the sign-in flow (disable re-authentication for
/// password-less accounts).
pub async fn claim_factor_standalone(
    pool: &PgPool,
    user_id: Uuid,
    factor: SecondFactor,
    recovery_hash: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let claimed = claim_factor(&mut tx, user_id, factor, recovery_hash).await?;
    if claimed {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(claimed)
}

/// Loaded state for [`complete_challenge`]: the row and generation the caller
/// verified the code against.
pub struct ChallengeCheck<'a> {
    pub token_hash: &'a str,
    pub user_id: Uuid,
    pub generation: i32,
    /// The sealed secret the code was verified against.
    pub secret: &'a str,
    pub factor: SecondFactor,
    pub recovery_hash: Option<&'a str>,
}

/// Source `completeMfaChallenge` transaction: under the sign-in lock the
/// challenge, account and MFA row must be unchanged; the factor is claimed,
/// the challenge consumed and the session issued together.
pub async fn complete_challenge(
    pool: &PgPool,
    check: ChallengeCheck<'_>,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, check.user_id).await?;
    let again: Option<(Uuid, i32)> =
        sqlx::query_as("SELECT user_id, generation FROM fvoci.app_mfa_challenge_peek($1)")
            .bind(check.token_hash)
            .fetch_optional(&mut *tx)
            .await?;
    let still = again == Some((check.user_id, check.generation))
        && live_generation(&mut tx, check.user_id).await? == Some(check.generation);
    let row = if still {
        find_in_tx(&mut tx, check.user_id).await?
    } else {
        None
    };
    let row_ok = row.is_some_and(|r| r.enabled_at.is_some() && r.totp_secret == check.secret);
    if !row_ok || !claim_factor(&mut tx, check.user_id, check.factor, check.recovery_hash).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let consumed: bool = sqlx::query_scalar("SELECT fvoci.app_mfa_challenge_consume($1, $2)")
        .bind(check.token_hash)
        .bind(check.user_id)
        .fetch_one(&mut *tx)
        .await?;
    if !consumed {
        tx.rollback().await?;
        return Ok(None);
    }
    let token = issue_session_in_tx(&mut tx, check.user_id, check.factor.method()).await?;
    tx.commit().await?;
    Ok(Some((check.user_id, token)))
}

/// Maintenance GC of expired challenges and OIDC flow state.
pub async fn purge_expired_ephemeral(
    pool: &PgPool,
    now: DateTime<Utc>,
    limit: i32,
) -> Result<u32, sqlx::Error> {
    let deleted: i32 = sqlx::query_scalar("SELECT fvoci.app_auth_ephemeral_purge_expired($1, $2)")
        .bind(now)
        .bind(limit)
        .fetch_one(pool)
        .await?;
    Ok(deleted.max(0) as u32)
}
