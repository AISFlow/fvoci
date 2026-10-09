//! TOTP MFA storage and the single sign-in gate.
//!
//! Source `packages/core/src/mfa.ts`. Every first factor (password, magic
//! link, invitation, OIDC) ends in [`issue_session_or_challenge`]: an account
//! with MFA enabled gets a 5-minute challenge token instead of a session, and
//! only [`complete_challenge`] turns it into one.

use super::backend::{Backend, OperationTx};
use super::codec::Cell;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token::{new_token, SESSION_TTL_SECS};
use crate::db::account::{record_account_change, AccountRecord};
use crate::db::context::{clear_self_user, recheck_session, set_self_user};
use crate::db::identity::{lock_sign_in, EventAppend};

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
///
/// `same_secret` tells whether a stored (sealed) value holds the secret the
/// code was checked against. The sealed text alone cannot tell: a concurrent
/// setup replaces the secret, but `--secrets-rotate` re-seals the same one.
pub async fn enable(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    same_secret: impl Fn(&str) -> bool,
    step: i64,
    ip: Option<&str>,
) -> Result<EnableOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(EnableOutcome::SessionGone);
    }
    set_self_user(&mut tx, user_id).await?;
    // Locked, so neither setup nor a re-seal can change it before the UPDATE.
    let current: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT totp_secret FROM fvoci.user_mfa
        WHERE user_id = $1 AND enabled_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let enabled = match current.filter(|(sealed,)| same_secret(sealed)) {
        Some((sealed,)) => {
            sqlx::query(
                r#"
                UPDATE fvoci.user_mfa
                SET enabled_at = now(), last_used_step = $2, updated_at = now()
                WHERE user_id = $1 AND enabled_at IS NULL AND totp_secret = $3
                "#,
            )
            .bind(user_id)
            .bind(step as i32)
            .bind(&sealed)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1
        }
        None => false,
    };
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
    issue_session_operation(&mut OperationTx::Postgres(tx), user_id, method).await
}

async fn issue_session_operation(
    tx: &mut OperationTx<'_, '_>,
    user_id: Uuid,
    method: &str,
) -> Result<String, sqlx::Error> {
    let token = new_token();
    let expires_at = crate::db::identity::stored_now() + Duration::seconds(SESSION_TTL_SECS);
    tx.create_session(Uuid::now_v7(), user_id, &token.hash, expires_at)
        .await?;
    let previous = tx.set_system().await?;
    tx.append_event(EventAppend {
        id: Uuid::now_v7(),
        workspace_id: None,
        actor_user_id: Some(user_id),
        verb: "auth.login".into(),
        target_type: Some("user".into()),
        target_id: Some(user_id),
        payload: json!({"userId":user_id.to_string(),"method":method}),
    })
    .await?;
    tx.restore_system(previous).await?;
    Ok(token.token)
}

/// Shared current-account/MFA first-factor gate, with the reservation/lock
/// acquired before reading auth_generation or the enabled MFA state.
pub async fn issue_session_or_challenge(
    pool: &PgPool,
    user_id: Uuid,
    method: &str,
    opts: IssueOptions,
) -> Result<Option<Issued>, sqlx::Error> {
    issue_session_or_challenge_backend(&Backend::Postgres(pool.clone()), user_id, method, opts)
        .await
}

pub async fn issue_session_or_challenge_backend(
    backend: &Backend,
    user_id: Uuid,
    method: &str,
    opts: IssueOptions,
) -> Result<Option<Issued>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut operation = tx.operation();
    operation.lock_sign_in(user_id).await?;
    let Some(generation) = operation.live_auth_generation(user_id).await? else {
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
    if opts.mark_email_verified && !operation.mark_email_verified(user_id).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    if !operation.mfa_is_enabled(user_id).await? {
        let token = issue_session_operation(&mut operation, user_id, method).await?;
        tx.commit().await.map_err(|unknown| unknown.source)?;
        return Ok(Some(Issued::Session { user_id, token }));
    }
    let challenge = new_token();
    operation
        .issue_mfa_challenge(
            &challenge.hash,
            user_id,
            generation,
            crate::db::identity::stored_now() + Duration::seconds(MFA_CHALLENGE_TTL_SECS),
        )
        .await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Some(Issued::Challenge {
        mfa_token: challenge.token,
    }))
}

/// The pending challenge's owner, without consuming it.
/// Counts one verify attempt for the account in the database (shared across
/// processes and restarts). `Ok(None)` when allowed, otherwise the seconds
/// until the window ends.
pub async fn verify_attempt(
    pool: &PgPool,
    user_id: Uuid,
    limit: u32,
    window_seconds: u32,
) -> Result<Option<u32>, sqlx::Error> {
    let retry: i32 = sqlx::query_scalar("SELECT fvoci.app_mfa_verify_attempt($1, $2, $3)")
        .bind(user_id)
        .bind(limit as i32)
        .bind(window_seconds as i32)
        .fetch_one(pool)
        .await?;
    Ok((retry > 0).then_some(retry as u32))
}

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
    /// `enabled_at` of the MFA row the code was verified against. It marks
    /// that secret: disable deletes the row and a later enable stamps a new
    /// time, while re-sealing the same secret (`--secrets-rotate`) keeps it,
    /// so the sealed text itself is not compared.
    pub enabled_at: DateTime<Utc>,
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
    let row_ok = row.is_some_and(|r| r.enabled_at == Some(check.enabled_at));
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

impl OperationTx<'_, '_> {
    async fn live_auth_generation(&mut self, user: Uuid) -> Result<Option<i32>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => live_generation(tx, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                let rows=tx.query("SELECT auth_generation FROM users WHERE id=?1 AND deleted_at IS NULL AND suspended_at IS NULL LIMIT 1", &[Cell::uuid(user)]).await?;
                rows.first()
                    .map(|row| {
                        i32::try_from(row.cell(0)?.integer()?).map_err(|_| {
                            sqlx::Error::Protocol("stored auth generation exceeds i32".into())
                        })
                    })
                    .transpose()
            }
        }
    }
    async fn mark_email_verified(&mut self, user: Uuid) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT fvoci.app_user_mark_email_verified($1)")
                    .bind(user)
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                Ok(tx.execute("UPDATE users SET email_verified_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000),updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND deleted_at IS NULL", &[Cell::uuid(user)]).await?==1)
            }
        }
    }
    async fn mfa_is_enabled(&mut self, user: Uuid) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(find_in_tx(tx, user)
                .await?
                .is_some_and(|row| row.enabled_at.is_some())),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.query("SELECT EXISTS(SELECT 1 FROM user_mfa WHERE user_id=?1 AND enabled_at IS NOT NULL)", &[Cell::uuid(user)]).await?.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.boolean()
            }
        }
    }
    async fn issue_mfa_challenge(
        &mut self,
        hash: &str,
        user: Uuid,
        generation: i32,
        expires: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("SELECT fvoci.app_mfa_challenge_issue($1,$2,$3,$4)")
                    .bind(hash)
                    .bind(user)
                    .bind(generation)
                    .bind(expires)
                    .execute(&mut ***tx)
                    .await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.execute("INSERT INTO mfa_challenges (token_hash,user_id,generation,expires_at) VALUES (?1,?2,?3,?4)", &[Cell::text(hash),Cell::uuid(user),Cell::Integer(i64::from(generation)),Cell::instant(expires)?]).await?;
            }
        }
        Ok(())
    }
}
