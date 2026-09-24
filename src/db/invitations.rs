use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::password::{hash_password, verify_password, Keyring};
use crate::auth::token::{hash_token, new_token};
use crate::db::context::{
    clear_invitation_token_hash, lock_membership_users, recheck_session, set_invitation_token_hash,
    set_tenant,
};
use crate::db::identity::{
    find_user_id_by_email, issue_session, password_hash_by_id, rehash_password_if_unchanged,
};
use crate::db::quota::{acquire_admission_lock, require_membership_admission, QuotaError};
use crate::db::workspace::{
    record_workspace_event_and_audit, WorkspaceChangeRecord, WorkspaceRole,
};

const INVITE_TTL: Duration = Duration::days(7);

#[derive(Debug)]
pub enum InvitationDbError {
    NotFound,
    Forbidden,
    PersonalImmutable,
    RoleCap,
    Expired,
    AlreadyAccepted,
    Unauthorized,
    ConsentRequired,
    SeatLimit,
    GuestLimit,
}

pub struct InvitationRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub email: String,
    pub role: WorkspaceRole,
    pub token_hash: String,
    pub invited_by: Uuid,
    pub expires_at: chrono::DateTime<Utc>,
    pub accepted_at: Option<chrono::DateTime<Utc>>,
}

pub struct InvitationPublic {
    pub workspace_name: String,
    pub email_masked: String,
    pub role: WorkspaceRole,
}

pub struct CreatedInvitation {
    pub accept_path_token: String,
}

pub struct AcceptInvitationRequest<'a> {
    pub email: Option<&'a str>,
    pub given_name: Option<&'a str>,
    pub family_name: Option<&'a str>,
    pub password: Option<&'a str>,
    pub client_ip: Option<&'a str>,
}

type InvitationScan = (
    Uuid,
    Uuid,
    String,
    String,
    String,
    Uuid,
    chrono::DateTime<Utc>,
    Option<chrono::DateTime<Utc>>,
);

struct NewAccount {
    email: String,
    given_name: String,
    family_name: Option<String>,
    password_hash: String,
}

pub async fn remove_pending_by_inviter(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    invited_by: Uuid,
    roles: &[WorkspaceRole],
) -> Result<i64, sqlx::Error> {
    if roles.is_empty() {
        return Ok(0);
    }
    let role_names: Vec<&str> = roles.iter().map(|role| role.as_str()).collect();
    let result = sqlx::query(
        r#"
        DELETE FROM fvoci.invitations
        WHERE workspace_id = $1
          AND invited_by = $2
          AND accepted_at IS NULL
          AND role = ANY($3)
        "#,
    )
    .bind(workspace_id)
    .bind(invited_by)
    .bind(&role_names)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() as i64)
}

pub async fn create_invitation(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    email: &str,
    role: WorkspaceRole,
    client_ip: Option<&str>,
) -> Result<Result<CreatedInvitation, InvitationDbError>, sqlx::Error> {
    let token = new_token();
    let invitation_id = Uuid::now_v7();
    let expires_at = Utc::now() + INVITE_TTL;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::Forbidden));
    }
    if !user_is_present(&mut tx, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    let kind = lock_workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    let inviter_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let inviter_role = match inviter_role {
        Some(role) if role.at_least(WorkspaceRole::Admin) => role,
        Some(_) => {
            tx.rollback().await?;
            return Ok(Err(InvitationDbError::Forbidden));
        }
        None => {
            tx.rollback().await?;
            return Ok(Err(InvitationDbError::NotFound));
        }
    };
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::PersonalImmutable));
    }
    if !inviter_role.at_least(role) {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::RoleCap));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.invitations (
            id, workspace_id, email, role, token_hash, invited_by, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(invitation_id)
    .bind(workspace_id)
    .bind(email)
    .bind(role.as_str())
    .bind(&token.hash)
    .bind(actor_user_id)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "invitation.created",
            target_type: "invitation",
            target_id: invitation_id,
            payload: json!({
                "workspaceId": workspace_id.to_string(),
                "email": email,
                "invitationId": invitation_id.to_string(),
            }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(CreatedInvitation {
        accept_path_token: token.token,
    }))
}

pub async fn get_invitation_public(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Result<InvitationPublic, InvitationDbError>, sqlx::Error> {
    let invitation = match load_invitation_by_token(pool, raw_token).await? {
        Ok(row) => row,
        Err(err) => return Ok(Err(err)),
    };
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, invitation.workspace_id).await?;
    let workspace = sqlx::query_as::<_, (String, String)>(
        "SELECT name, kind FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(invitation.workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((name, kind)) = workspace else {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    };
    if kind == "personal" {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    if !inviter_still_authorized(&mut tx, &invitation).await? {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(InvitationPublic {
        workspace_name: name,
        email_masked: mask_email(&invitation.email),
        role: invitation.role,
    }))
}

pub async fn accept_invitation(
    pool: &PgPool,
    keys: &Keyring,
    raw_token: &str,
    request: AcceptInvitationRequest<'_>,
) -> Result<Result<(Uuid, String), InvitationDbError>, sqlx::Error> {
    let invitation = match load_invitation_by_token(pool, raw_token).await? {
        Ok(row) => row,
        Err(err) => return Ok(Err(err)),
    };
    if let Some(email) = request.email {
        if email != invitation.email {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
    }

    // Legal documents are not ported; source coverage is vacuously true when
    // listRequiredLatest is empty, so 428 is wired but not reachable yet.
    let existing = find_user_id_by_email(pool, &invitation.email).await?;
    let mut new_account = None;
    let user_id;
    let mut rehash = None;

    if let Some((existing_id, suspended_at)) = existing {
        let stored = password_hash_by_id(pool, existing_id).await?;
        let verified =
            verify_password(stored.as_deref(), request.password.unwrap_or(""), keys).await;
        if !verified.ok || stored.is_none() {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        if suspended_at.is_some() {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        user_id = existing_id;
        if verified.needs_pepper_rotation {
            if let Ok(replacement) = hash_password(request.password.unwrap_or(""), keys).await {
                rehash = Some((stored.unwrap(), replacement));
            }
        }
    } else {
        if email_exists(pool, &invitation.email).await? {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        let (Some(email), Some(given_name), Some(password)) =
            (request.email, request.given_name, request.password)
        else {
            return Ok(Err(InvitationDbError::Unauthorized));
        };
        let password_hash = match hash_password(password, keys).await {
            Ok(hash) => hash,
            Err(_) => return Ok(Err(InvitationDbError::Unauthorized)),
        };
        user_id = Uuid::now_v7();
        new_account = Some(NewAccount {
            email: email.to_string(),
            given_name: given_name.to_string(),
            family_name: request
                .family_name
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            password_hash,
        });
    }

    if let Err(err) =
        grant_membership(pool, &invitation, user_id, new_account, request.client_ip).await?
    {
        return Ok(Err(err));
    }
    if let Some((expected, replacement)) = rehash {
        let _ = rehash_password_if_unchanged(pool, user_id, &replacement, &expected).await;
    }
    match issue_session(pool, user_id).await? {
        Some(token) => Ok(Ok((user_id, token))),
        None => Ok(Err(InvitationDbError::Unauthorized)),
    }
}

async fn grant_membership(
    pool: &PgPool,
    invitation: &InvitationRow,
    user_id: Uuid,
    new_account: Option<NewAccount>,
    client_ip: Option<&str>,
) -> Result<Result<(), InvitationDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    set_tenant(&mut tx, invitation.workspace_id).await?;
    lock_membership_users(&mut tx, &[invitation.invited_by, user_id]).await?;
    let kind = lock_workspace_kind(&mut tx, invitation.workspace_id).await?;
    if kind.is_none() || kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    let current = lock_invitation(&mut tx, invitation.workspace_id, invitation.id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    };
    if current.token_hash != invitation.token_hash || current.email != invitation.email {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::NotFound));
    }
    if current.accepted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::AlreadyAccepted));
    }
    if current.expires_at <= Utc::now() {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::Expired));
    }
    if !inviter_still_authorized(&mut tx, &current).await? {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::Unauthorized));
    }
    if let Some(new_account) = &new_account {
        if new_account.email != current.email {
            tx.rollback().await?;
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        sqlx::query(
            r#"
            INSERT INTO fvoci.users (id, email, password_hash, given_name, family_name)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(user_id)
        .bind(&new_account.email)
        .bind(&new_account.password_hash)
        .bind(&new_account.given_name)
        .bind(&new_account.family_name)
        .execute(&mut *tx)
        .await?;
    } else {
        let existing = sqlx::query_as::<_, (String,)>(
            "SELECT email FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        match existing {
            Some((email,)) if email == current.email => {}
            _ => {
                tx.rollback().await?;
                return Ok(Err(InvitationDbError::Unauthorized));
            }
        }
    }
    let already_member = membership_role(&mut tx, invitation.workspace_id, user_id)
        .await?
        .is_some();
    if !already_member {
        if let Err(err) = require_membership_admission(&mut tx, user_id, current.role, None).await?
        {
            tx.rollback().await?;
            return Ok(Err(quota_error(err)));
        }
        sqlx::query(
            "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)",
        )
        .bind(invitation.workspace_id)
        .bind(user_id)
        .bind(current.role.as_str())
        .execute(&mut *tx)
        .await?;
    }
    let marked = sqlx::query_as::<_, (Uuid,)>(
        r#"
        UPDATE fvoci.invitations
        SET accepted_at = now(), updated_at = now()
        WHERE id = $1 AND workspace_id = $2 AND accepted_at IS NULL
        RETURNING id
        "#,
    )
    .bind(current.id)
    .bind(invitation.workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    if marked.is_none() {
        tx.rollback().await?;
        return Ok(Err(InvitationDbError::AlreadyAccepted));
    }
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id: invitation.workspace_id,
            actor_user_id: user_id,
            verb: "invitation.accepted",
            target_type: "invitation",
            target_id: current.id,
            payload: json!({
                "workspaceId": invitation.workspace_id.to_string(),
                "userId": user_id.to_string(),
                "email": current.email,
            }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

fn quota_error(err: QuotaError) -> InvitationDbError {
    match err {
        QuotaError::SeatLimit => InvitationDbError::SeatLimit,
        QuotaError::GuestLimit => InvitationDbError::GuestLimit,
    }
}

async fn load_invitation_by_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Result<InvitationRow, InvitationDbError>, sqlx::Error> {
    let token_hash = hash_token(raw_token);
    let mut tx = pool.begin().await?;
    set_invitation_token_hash(&mut tx, &token_hash).await?;
    let row = fetch_invitation_by_hash(&mut tx, &token_hash).await?;
    clear_invitation_token_hash(&mut tx).await?;
    tx.commit().await?;
    let Some(invitation) = row else {
        return Ok(Err(InvitationDbError::NotFound));
    };
    if invitation.accepted_at.is_some() {
        return Ok(Err(InvitationDbError::AlreadyAccepted));
    }
    if invitation.expires_at <= Utc::now() {
        return Ok(Err(InvitationDbError::Expired));
    }
    Ok(Ok(invitation))
}

async fn fetch_invitation_by_hash(
    tx: &mut Transaction<'_, Postgres>,
    token_hash: &str,
) -> Result<Option<InvitationRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, InvitationScan>(
        r#"
        SELECT id, workspace_id, email, role, token_hash, invited_by, expires_at, accepted_at
        FROM fvoci.invitations
        WHERE token_hash = $1
        "#,
    )
    .bind(token_hash)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(parse_invitation_row))
}

async fn lock_invitation(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    invitation_id: Uuid,
) -> Result<Option<InvitationRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, InvitationScan>(
        r#"
        SELECT id, workspace_id, email, role, token_hash, invited_by, expires_at, accepted_at
        FROM fvoci.invitations
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(invitation_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(parse_invitation_row))
}

fn parse_invitation_row(row: InvitationScan) -> Option<InvitationRow> {
    let (id, workspace_id, email, role, token_hash, invited_by, expires_at, accepted_at) = row;
    WorkspaceRole::parse(&role).map(|role| InvitationRow {
        id,
        workspace_id,
        email,
        role,
        token_hash,
        invited_by,
        expires_at,
        accepted_at,
    })
}

async fn inviter_still_authorized(
    tx: &mut Transaction<'_, Postgres>,
    invitation: &InvitationRow,
) -> Result<bool, sqlx::Error> {
    if !user_is_present(tx, invitation.invited_by).await? {
        return Ok(false);
    }
    let inviter = membership_role(tx, invitation.workspace_id, invitation.invited_by).await?;
    Ok(inviter
        .map(|role| role.at_least(WorkspaceRole::Admin) && role.at_least(invitation.role))
        .unwrap_or(false))
}

async fn lock_workspace_kind(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<chrono::DateTime<Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match row {
        Some((kind, deleted)) if deleted.is_none() => Ok(kind),
        _ => Ok(None),
    }
}

async fn membership_role(
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

async fn user_is_present(
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

async fn email_exists(pool: &PgPool, email: &str) -> Result<bool, sqlx::Error> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.users WHERE email = $1)")
            .bind(email)
            .fetch_one(pool)
            .await?;
    Ok(exists)
}

fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) if !local.is_empty() => {
            format!("{}***@{}", local.chars().next().unwrap_or('*'), domain)
        }
        _ => format!("***{email}"),
    }
}

#[cfg(feature = "db-tests")]
pub async fn expire_invitation_for_test(
    pool: &PgPool,
    workspace_id: Uuid,
    token_hash: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    sqlx::query(
        "UPDATE fvoci.invitations SET expires_at = now() - interval '1 second' WHERE token_hash = $1",
    )
    .bind(token_hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(feature = "db-tests")]
pub async fn revoke_pending_invitation_for_test(
    pool: &PgPool,
    workspace_id: Uuid,
    token_hash: &str,
) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result =
        sqlx::query("DELETE FROM fvoci.invitations WHERE token_hash = $1 AND accepted_at IS NULL")
            .bind(token_hash)
            .execute(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}
