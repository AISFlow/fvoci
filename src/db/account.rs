//! Account lifecycle writes: withdraw / cancel / final anonymization, password
//! change, email change and magic-link login.
//!
//! Source: packages/core/src/consent.ts (withdrawUser, cancelUserErasure,
//! anonymizeWithdrawnUsers) and packages/core/src/magic-link.ts. Lock order
//! matches the source: admission lock -> instance-admin lock -> membership
//! lock -> users row (`lockSignIn`). Every mutation writes its event and audit
//! row in the same transaction.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token::{hash_token, new_token, token_hashes_eq, SESSION_TTL_SECS};
use crate::db::context::{
    clear_self_user, lock_membership_users, recheck_session, set_self_user, set_system, set_tenant,
};
use crate::db::identity::{
    append_audit, create_session, lock_sign_in, AuditAppend, INSTANCE_ADMIN_LOCK_KEY,
};
use crate::db::magic::{MagicPayload, MAGIC_KIND_EMAIL_CHANGE, MAGIC_KIND_LOGIN};
use crate::db::quota::acquire_admission_lock;
use crate::validate::normalize_email;

/// Source `ERASE_AFTER_MS`: 14 days between withdraw and anonymization.
pub const WITHDRAW_GRACE_DAYS: i64 = 14;
/// Source `WITHDRAWN_ANONYMIZE_BATCH`.
pub const WITHDRAWN_ANONYMIZE_BATCH: i64 = 200;
/// Source `withdrawn.displayName` (ko).
pub const WITHDRAWN_DISPLAY_NAME: &str = "탈퇴한 사용자";
/// Source `scrubNamesByUploader(user.id, "deleted")`.
pub const SCRUBBED_ATTACHMENT_NAME: &str = "deleted";

pub fn withdrawal_deadline(deleted_at: DateTime<Utc>) -> DateTime<Utc> {
    deleted_at + Duration::days(WITHDRAW_GRACE_DAYS)
}

#[derive(Debug, Clone)]
pub enum WithdrawConfirm {
    /// Password accounts: the current password (verified by the caller).
    Password { verified_hash: String },
    /// Password-less accounts: the local part of the current email.
    EmailLocalPart(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum WithdrawError {
    ConfirmInvalid,
    OwnerTransferRequired,
    LastInstanceAdmin,
    SessionGone,
}

#[derive(Debug, Clone)]
pub struct WithdrawScheduled {
    pub cancel_token: String,
    pub erase_at: DateTime<Utc>,
    pub email: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CancelWithdrawOutcome {
    Ok,
    NotFound,
    DeadlinePassed,
}

/// Printable ASCII only — no trim/NFKC/Unicode alias folding (source
/// `WITHDRAW_EMAIL_LOCAL_ASCII`).
pub fn email_local_part_matches(stored_email: &str, submitted: &str) -> bool {
    if submitted.is_empty() || !submitted.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return false;
    }
    if submitted.contains('@') {
        return false;
    }
    let Some(at) = stored_email.find('@') else {
        return false;
    };
    if at == 0 || at >= stored_email.len() - 1 {
        return false;
    }
    let Ok(stored) = normalize_email(stored_email) else {
        return false;
    };
    let candidate = format!("{submitted}@{}", &stored_email[at + 1..]);
    let Ok(confirmed) = normalize_email(&candidate) else {
        return false;
    };
    token_hashes_eq(&confirmed, &stored)
}

async fn lock_account(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    acquire_admission_lock(tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(INSTANCE_ADMIN_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    lock_membership_users(tx, &[user_id]).await?;
    lock_sign_in(tx, user_id).await
}

struct AccountRow {
    email: String,
    deleted_at: Option<DateTime<Utc>>,
    anonymized_at: Option<DateTime<Utc>>,
    is_instance_admin: bool,
    personal_workspace_id: Option<Uuid>,
}

async fn account_row(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<AccountRow>, sqlx::Error> {
    type Row = (
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        bool,
        Option<Uuid>,
    );
    let row: Option<Row> = sqlx::query_as(
        r#"
        SELECT email, deleted_at, anonymized_at, is_instance_admin, personal_workspace_id
        FROM fvoci.users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(email, deleted_at, anonymized_at, is_instance_admin, personal_workspace_id)| AccountRow {
            email,
            deleted_at,
            anonymized_at,
            is_instance_admin,
            personal_workspace_id,
        },
    ))
}

async fn password_hash_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT fvoci.app_user_password_hash($1)")
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.and_then(|(hash,)| hash))
}

fn same_hash(current: Option<&str>, expected: Option<&str>) -> bool {
    match (current, expected) {
        (Some(current), Some(expected)) => token_hashes_eq(current, expected),
        (None, None) => true,
        _ => false,
    }
}

async fn membership_workspaces(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    set_self_user(tx, user_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT workspace_id, role FROM fvoci.memberships WHERE user_id = $1 ORDER BY workspace_id",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    clear_self_user(tx).await?;
    Ok(rows)
}

/// Source `hasLiveTeamOwnership`: any `owner` membership of a live team workspace.
async fn has_live_team_ownership(
    tx: &mut Transaction<'_, Postgres>,
    memberships: &[(Uuid, String)],
) -> Result<bool, sqlx::Error> {
    let owned: Vec<Uuid> = memberships
        .iter()
        .filter(|(_, role)| role == "owner")
        .map(|(id, _)| *id)
        .collect();
    if owned.is_empty() {
        return Ok(false);
    }
    let previous = set_system(tx).await?;
    let live: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.workspaces
            WHERE id = ANY($1) AND kind = 'team' AND deleted_at IS NULL
        )
        "#,
    )
    .bind(&owned)
    .fetch_one(&mut **tx)
    .await?;
    crate::db::context::restore_system(tx, &previous).await?;
    Ok(live)
}

async fn count_live_instance_admins(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT count(*) FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
        "#,
    )
    .fetch_one(&mut **tx)
    .await
}

pub(crate) struct AccountRecord<'a> {
    pub verb: &'a str,
    pub actor_user_id: Option<Uuid>,
    pub target_id: Uuid,
    pub payload: Value,
    /// Audit copy of the payload when it must not repeat the event's personal
    /// data (audit rows outlive anonymization). `None` reuses `payload`.
    pub audit_payload: Option<Value>,
    pub ip: Option<&'a str>,
    pub channel: &'a str,
    pub workspace_id: Option<Uuid>,
    pub target_type: &'a str,
}

/// Event (outbox) + audit row in the caller's transaction.
pub(crate) async fn record_account_change(
    tx: &mut Transaction<'_, Postgres>,
    record: AccountRecord<'_>,
) -> Result<(), sqlx::Error> {
    let previous = set_system(tx).await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(record.workspace_id)
    .bind(record.actor_user_id)
    .bind(record.verb)
    .bind(record.target_type)
    .bind(record.target_id)
    .bind(&record.payload)
    .bind(record.channel)
    .execute(&mut **tx)
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: record.workspace_id,
            actor_user_id: record.actor_user_id,
            verb: record.verb.to_string(),
            target_type: Some(record.target_type.to_string()),
            target_id: Some(record.target_id),
            payload: record.audit_payload.unwrap_or(record.payload),
            ip: record.ip.map(str::to_string),
        },
    )
    .await?;
    crate::db::context::restore_system(tx, &previous).await
}

fn user_record<'a>(
    verb: &'a str,
    actor: Option<Uuid>,
    user_id: Uuid,
    payload: Value,
    ip: Option<&'a str>,
) -> AccountRecord<'a> {
    AccountRecord {
        verb,
        actor_user_id: actor,
        target_id: user_id,
        payload,
        audit_payload: None,
        ip,
        channel: if actor.is_some() { "web" } else { "system" },
        workspace_id: None,
        target_type: "user",
    }
}

async fn revoke_user_credentials(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    crate::db::identity::revoke_all_sessions_for_user(tx, user_id).await?;
    let previous = set_system(tx).await?;
    sqlx::query("DELETE FROM fvoci.api_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM fvoci.ics_tokens WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    crate::db::context::restore_system(tx, &previous).await
}

/// Source `withdrawUser`. The caller verifies a password confirmation outside
/// the transaction; here the stored hash must still be the verified one.
pub async fn withdraw_user(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    confirm: &WithdrawConfirm,
    ip: Option<&str>,
) -> Result<Result<WithdrawScheduled, WithdrawError>, sqlx::Error> {
    let cancel = new_token();
    let mut tx = pool.begin().await?;
    lock_account(&mut tx, user_id).await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::SessionGone));
    }
    let Some(current) = account_row(&mut tx, user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::ConfirmInvalid));
    };
    if current.anonymized_at.is_some() || current.deleted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::ConfirmInvalid));
    }
    let stored_hash = password_hash_in_tx(&mut tx, user_id).await?;
    let confirmed = match confirm {
        WithdrawConfirm::Password { verified_hash } => {
            same_hash(stored_hash.as_deref(), Some(verified_hash))
        }
        WithdrawConfirm::EmailLocalPart(local) => {
            stored_hash.is_none() && email_local_part_matches(&current.email, local)
        }
    };
    if !confirmed {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::ConfirmInvalid));
    }
    let memberships = membership_workspaces(&mut tx, user_id).await?;
    if has_live_team_ownership(&mut tx, &memberships).await? {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::OwnerTransferRequired));
    }
    if current.is_instance_admin && count_live_instance_admins(&mut tx).await? <= 1 {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::LastInstanceAdmin));
    }
    for (workspace_id, _) in &memberships {
        set_tenant(&mut tx, *workspace_id).await?;
        sqlx::query(
            r#"
            DELETE FROM fvoci.invitations
            WHERE workspace_id = $1 AND invited_by = $2 AND accepted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    let at = Utc::now();
    let marked: bool = sqlx::query_scalar("SELECT fvoci.app_user_withdraw($1, $2, $3)")
        .bind(user_id)
        .bind(at)
        .bind(&cancel.hash)
        .fetch_one(&mut *tx)
        .await?;
    if !marked {
        tx.rollback().await?;
        return Ok(Err(WithdrawError::ConfirmInvalid));
    }
    revoke_user_credentials(&mut tx, user_id).await?;
    record_account_change(
        &mut tx,
        user_record(
            "user.withdrawn",
            Some(user_id),
            user_id,
            json!({ "userId": user_id.to_string() }),
            ip,
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WithdrawScheduled {
        cancel_token: cancel.token,
        erase_at: withdrawal_deadline(at),
        email: current.email,
    }))
}

async fn user_id_by_cancel_hash(
    executor: impl sqlx::PgExecutor<'_>,
    hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_user_id_by_withdraw_cancel_token_hash($1)")
        .bind(hash)
        .fetch_one(executor)
        .await
}

/// Source `cancelUserErasure({ token })`.
pub async fn cancel_withdraw(
    pool: &PgPool,
    token: &str,
    ip: Option<&str>,
) -> Result<CancelWithdrawOutcome, sqlx::Error> {
    let hash = hash_token(token);
    let Some(user_id) = user_id_by_cancel_hash(pool, &hash).await? else {
        return Ok(CancelWithdrawOutcome::NotFound);
    };
    let mut tx = pool.begin().await?;
    lock_account(&mut tx, user_id).await?;
    if user_id_by_cancel_hash(&mut *tx, &hash).await? != Some(user_id) {
        tx.rollback().await?;
        return Ok(CancelWithdrawOutcome::NotFound);
    }
    let Some(current) = account_row(&mut tx, user_id).await? else {
        tx.rollback().await?;
        return Ok(CancelWithdrawOutcome::NotFound);
    };
    let Some(deleted_at) = current
        .deleted_at
        .filter(|_| current.anonymized_at.is_none())
    else {
        tx.rollback().await?;
        return Ok(CancelWithdrawOutcome::NotFound);
    };
    // Wall clock after the row lock: a pre-lock `now` could cancel past the deadline.
    if Utc::now() >= withdrawal_deadline(deleted_at) {
        tx.rollback().await?;
        return Ok(CancelWithdrawOutcome::DeadlinePassed);
    }
    let restored: bool = sqlx::query_scalar("SELECT fvoci.app_user_restore_withdrawn($1, $2)")
        .bind(user_id)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await?;
    if !restored {
        tx.rollback().await?;
        return Ok(CancelWithdrawOutcome::NotFound);
    }
    record_account_change(
        &mut tx,
        user_record(
            "user.withdraw_cancelled",
            Some(user_id),
            user_id,
            json!({ "userId": user_id.to_string() }),
            ip,
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(CancelWithdrawOutcome::Ok)
}

/// Source `markPersonalWorkspaceDeleted`; the workspace purge job removes it.
async fn mark_personal_workspace_deleted(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    let locked: Option<(String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match locked {
        Some((kind, None)) if kind == "personal" => {}
        _ => return Ok(()),
    }
    crate::db::invitations::remove_by_workspace(tx, workspace_id).await?;
    let previous = set_system(tx).await?;
    sqlx::query("DELETE FROM fvoci.ics_tokens WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut **tx)
        .await?;
    crate::db::context::restore_system(tx, &previous).await?;
    crate::db::api_tokens::remove_by_workspace(tx, workspace_id).await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE fvoci.workspaces SET deleted_at = now(), updated_at = now() WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .execute(&mut **tx)
    .await?;
    record_account_change(
        tx,
        AccountRecord {
            verb: "workspace.deleted",
            actor_user_id: None,
            target_id: workspace_id,
            payload: json!({}),
            audit_payload: None,
            ip: None,
            channel: "system",
            workspace_id: Some(workspace_id),
            target_type: "workspace",
        },
    )
    .await
}

fn anonymized_email() -> String {
    let id = Uuid::now_v7().simple().to_string();
    format!("withdrawn-{}@withdrawn.invalid", &id[id.len() - 12..])
}

/// Candidates whose grace period ended at or before `cutoff`. A snapshot, not
/// an authorization: each row is locked and rechecked before anonymization.
pub async fn list_withdrawn_due(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT id FROM fvoci.users
        WHERE deleted_at IS NOT NULL AND deleted_at <= $1 AND anonymized_at IS NULL
        ORDER BY deleted_at ASC, id ASC
        LIMIT $2
        "#,
    )
    .bind(cutoff)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Source `anonymizeWithdrawnUsers` per-user claim: all or nothing in one
/// transaction. Returns false when the row no longer qualifies.
pub async fn anonymize_withdrawn_user(
    pool: &PgPool,
    user_id: Uuid,
    now: DateTime<Utc>,
    cutoff: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_account(&mut tx, user_id).await?;
    let Some(current) = account_row(&mut tx, user_id).await? else {
        tx.rollback().await?;
        return Ok(false);
    };
    match current.deleted_at {
        Some(deleted_at) if current.anonymized_at.is_none() && deleted_at <= cutoff => {}
        _ => {
            tx.rollback().await?;
            return Ok(false);
        }
    }
    let anonymized: bool =
        sqlx::query_scalar("SELECT fvoci.app_user_anonymize($1, $2, $3, $4, $5)")
            .bind(user_id)
            .bind(WITHDRAWN_DISPLAY_NAME)
            .bind(anonymized_email())
            .bind(now)
            .bind(cutoff)
            .fetch_one(&mut *tx)
            .await?;
    if !anonymized {
        tx.rollback().await?;
        return Ok(false);
    }
    let previous = set_system(&mut tx).await?;
    sqlx::query("DELETE FROM fvoci.notifications WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    crate::db::context::restore_system(&mut tx, &previous).await?;
    sqlx::query("SELECT fvoci.app_attachments_scrub_uploader($1, $2)")
        .bind(user_id)
        .bind(SCRUBBED_ATTACHMENT_NAME)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM fvoci.sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    if let Some(workspace_id) = current.personal_workspace_id {
        mark_personal_workspace_deleted(&mut tx, workspace_id).await?;
    }
    record_account_change(
        &mut tx,
        user_record(
            "user.anonymized",
            None,
            user_id,
            json!({ "userId": user_id.to_string() }),
            None,
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Source `changePassword` transaction. `expected_hash` is the hash the caller
/// verified the current password against (`None` for a password-less account).
pub async fn change_password(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    expected_hash: Option<&str>,
    new_hash: &str,
    ip: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, user_id).await?;
    let live: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if !matches!(live, Some((None,))) {
        tx.rollback().await?;
        return Ok(false);
    }
    let current_hash = password_hash_in_tx(&mut tx, user_id).await?;
    if !same_hash(current_hash.as_deref(), expected_hash) {
        tx.rollback().await?;
        return Ok(false);
    }
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    crate::db::identity::set_password_hash(&mut tx, user_id, new_hash).await?;
    sqlx::query(
        r#"
        UPDATE fvoci.sessions
        SET revoked_at = now(), updated_at = now()
        WHERE user_id = $1 AND revoked_at IS NULL AND id <> $2
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .execute(&mut *tx)
    .await?;
    record_account_change(
        &mut tx,
        user_record(
            "auth.password_changed",
            Some(user_id),
            user_id,
            json!({ "userId": user_id.to_string() }),
            ip,
        ),
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub struct LiveUserIdentity {
    pub email: String,
    pub generation: i32,
    pub suspended: bool,
}

pub async fn live_user_identity(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<LiveUserIdentity>, sqlx::Error> {
    let row: Option<(String, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT email, auth_generation, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(email, generation, suspended_at)| LiveUserIdentity {
            email,
            generation,
            suspended: suspended_at.is_some(),
        }),
    )
}

/// Source `emailExists`: any row, withdrawn or anonymized included.
pub async fn email_exists(pool: &PgPool, email: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM fvoci.users WHERE email = $1)")
        .bind(email)
        .fetch_one(pool)
        .await
}

pub async fn issue_email_change_token(
    pool: &PgPool,
    user_id: Uuid,
    generation: i32,
    new_email: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    sqlx::query("SELECT fvoci.app_magic_issue_email_change($1, $2, $3, $4, $5)")
        .bind(token_hash)
        .bind(user_id)
        .bind(generation)
        .bind(new_email)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub struct EmailChanged {
    pub old_email: String,
}

/// Source `confirmEmailChange` transaction for an already consumed payload.
pub async fn complete_email_change(
    pool: &PgPool,
    payload: &MagicPayload,
    ip: Option<&str>,
) -> Result<Option<EmailChanged>, sqlx::Error> {
    if payload.kind != MAGIC_KIND_EMAIL_CHANGE {
        return Ok(None);
    }
    let Some(new_email) = payload.new_email.as_deref() else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, payload.user_id).await?;
    let row: Option<(String, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT email, auth_generation, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(payload.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((old_email, generation, suspended_at)) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    if suspended_at.is_some() || generation != payload.generation {
        tx.rollback().await?;
        return Ok(None);
    }
    let result: String = sqlx::query_scalar("SELECT fvoci.app_user_update_email($1, $2)")
        .bind(payload.user_id)
        .bind(new_email)
        .fetch_one(&mut *tx)
        .await?;
    if result != "ok" {
        tx.rollback().await?;
        return Ok(None);
    }
    // The outbox event keeps the source payload; the audit row, which is
    // never erased, records the change without either address.
    let mut record = user_record(
        "user.email_changed",
        Some(payload.user_id),
        payload.user_id,
        json!({
            "userId": payload.user_id.to_string(),
            "oldEmail": old_email,
            "newEmail": new_email,
        }),
        ip,
    );
    record.audit_payload = Some(json!({ "userId": payload.user_id.to_string() }));
    record_account_change(&mut tx, record).await?;
    tx.commit().await?;
    Ok(Some(EmailChanged { old_email }))
}

pub struct LoginLinkUser {
    pub user_id: Uuid,
    pub generation: i32,
}

/// Source `findByEmail` + `suspendedAt` check in `issueLinkMail`.
pub async fn login_link_user(
    pool: &PgPool,
    email: &str,
) -> Result<Option<LoginLinkUser>, sqlx::Error> {
    let row: Option<(Uuid, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, auth_generation, suspended_at FROM fvoci.users WHERE email = $1 AND deleted_at IS NULL",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(user_id, generation, suspended_at)| {
        suspended_at.is_none().then_some(LoginLinkUser {
            user_id,
            generation,
        })
    }))
}

pub async fn issue_login_token(
    pool: &PgPool,
    user: &LoginLinkUser,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    sqlx::query("SELECT fvoci.app_magic_issue($1, $2, $3, $4, $5)")
        .bind(token_hash)
        .bind(MAGIC_KIND_LOGIN)
        .bind(user.user_id)
        .bind(user.generation)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Source `issueSessionOrChallenge(..., "magic", ip, { generation,
/// markEmailVerified: true })` without MFA (not ported).
pub async fn complete_magic_login(
    pool: &PgPool,
    payload: &MagicPayload,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    if payload.kind != MAGIC_KIND_LOGIN {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, payload.user_id).await?;
    let row: Option<(i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT auth_generation, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(payload.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    match row {
        Some((generation, None)) if generation == payload.generation => {}
        _ => {
            tx.rollback().await?;
            return Ok(None);
        }
    }
    let verified: bool = sqlx::query_scalar("SELECT fvoci.app_user_mark_email_verified($1)")
        .bind(payload.user_id)
        .fetch_one(&mut *tx)
        .await?;
    if !verified {
        tx.rollback().await?;
        return Ok(None);
    }
    let token = new_token();
    let expires_at = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
    create_session(
        &mut tx,
        Uuid::now_v7(),
        payload.user_id,
        &token.hash,
        expires_at,
    )
    .await?;
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await?;
    crate::db::identity::append_event(
        &mut tx,
        crate::db::identity::EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(payload.user_id),
            verb: "auth.login".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(payload.user_id),
            payload: json!({ "userId": payload.user_id.to_string(), "method": "magic" }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Some((payload.user_id, token.token)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_local_part_is_ascii_and_canonical() {
        assert!(email_local_part_matches("kim@example.com", "kim"));
        assert!(email_local_part_matches("kim@example.com", "KIM"));
        assert!(!email_local_part_matches(
            "kim@example.com",
            "kim@example.com"
        ));
        assert!(!email_local_part_matches("kim@example.com", " kim"));
        assert!(!email_local_part_matches("kim@example.com", "김"));
        assert!(!email_local_part_matches("kim@example.com", "lee"));
        assert!(!email_local_part_matches("kim@example.com", ""));
    }

    #[test]
    fn deadline_is_fourteen_days() {
        let at = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            withdrawal_deadline(at).to_rfc3339(),
            "2026-09-15T00:00:00+00:00"
        );
    }

    #[test]
    fn anonymized_email_matches_definer_guard() {
        let email = anonymized_email();
        let re = regex::Regex::new(r"^withdrawn-[0-9a-f]{12}@withdrawn\.invalid$").unwrap();
        assert!(re.is_match(&email), "{email}");
    }
}
