use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::Cell;

use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::password::{hash_password, verify_password, Keyring};
use crate::auth::token::{hash_token, new_token};
use crate::db::context::{
    clear_invitation_token_hash, clear_self_user, lock_membership_users, recheck_session,
    set_invitation_token_hash, set_self_user, set_tenant,
};
use crate::db::identity::{
    find_user_id_by_email, password_hash_by_id, rehash_password_if_unchanged,
};
use crate::db::quota::{acquire_admission_lock, require_membership_admission, QuotaError};
use crate::db::workspace::{
    membership_role, record_workspace_event_and_audit, WorkspaceChangeRecord, WorkspaceRole,
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
    /// OIDC invite: the external identity already belongs to an account.
    AlreadyLinked,
    /// The password check for an existing account is over the caller's
    /// budget; seconds until it frees up.
    RateLimited(u32),
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
    /// Required latest legal documents the invitee must accept (kind, version, title).
    pub required_legal: Vec<(String, i32, String)>,
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
    /// Submitted `consents` items (kind, version).
    pub consents: &'a [(String, i32)],
    /// Instance `defaults.user` for a new account.
    pub defaults: &'a crate::settings::DefaultsUserSettings,
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
    /// None for an account created through an external identity.
    password_hash: Option<String>,
    defaults: crate::settings::DefaultsUserSettings,
}

struct GrantDetails<'a> {
    new_account: Option<NewAccount>,
    link: Option<&'a crate::db::oidc::NewLink<'a>>,
    client_ip: Option<&'a str>,
    consents: &'a [(String, i32)],
}

impl crate::db::backend::OperationTx<'_, '_> {
    pub(crate) async fn remove_pending_by_inviter(
        &mut self,
        workspace: Uuid,
        inviter: Uuid,
        roles: &[WorkspaceRole],
    ) -> Result<i64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => remove_pending_by_inviter(tx, workspace, inviter, roles).await,
            Self::SqliteFamily(tx) => {
                use crate::db::codec::Cell;
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                if roles.is_empty() {
                    return Ok(0);
                }
                let count = tx.execute(
                    "DELETE FROM invitations WHERE workspace_id=?1 AND invited_by=?2 AND accepted_at IS NULL AND ((?3=1 AND role='owner') OR (?4=1 AND role='admin') OR (?5=1 AND role='member') OR (?6=1 AND role='guest'))",
                    &[
                        Cell::uuid(workspace), Cell::uuid(inviter),
                        Cell::Integer(i64::from(roles.contains(&WorkspaceRole::Owner))),
                        Cell::Integer(i64::from(roles.contains(&WorkspaceRole::Admin))),
                        Cell::Integer(i64::from(roles.contains(&WorkspaceRole::Member))),
                        Cell::Integer(i64::from(roles.contains(&WorkspaceRole::Guest))),
                    ],
                ).await?;
                i64::try_from(count)
                    .map_err(|_| sqlx::Error::Protocol("invitation removal count overflow".into()))
            }
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_member_removal_invitation_tests {
    use super::*;
    use crate::db::workspace::selected_member_removal_tests::{fixture, invitation, snapshot};

    #[tokio::test]
    async fn sqlite_pending_inviter_roles_scope_accepted_and_empty_set() {
        let (f, _, target, _) = fixture().await;
        let owner = invitation(&f, f.workspace, target, "owner", false).await;
        let member = invitation(&f, f.workspace, target, "member", false).await;
        let accepted = invitation(&f, f.workspace, target, "member", true).await;
        let other = invitation(&f, f.workspace, f.user, "member", false).await;
        let foreign = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,'Foreign')")
            .bind(foreign.as_bytes().as_slice())
            .bind(foreign.simple().to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        let other_tenant = invitation(&f, foreign, target, "member", false).await;
        let before = snapshot(&f).await;
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_tenant(f.workspace).await.unwrap();
        assert!(read
            .operation()
            .remove_pending_by_inviter(f.workspace, target, &[WorkspaceRole::Member])
            .await
            .is_err());
        read.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        {
            let mut op = tx.operation();
            op.set_tenant(f.workspace).await.unwrap();
            assert_eq!(
                op.remove_pending_by_inviter(f.workspace, target, &[])
                    .await
                    .unwrap(),
                0
            );
            assert!(op
                .remove_pending_by_inviter(foreign, target, &[WorkspaceRole::Member])
                .await
                .is_err());
            assert_eq!(
                op.remove_pending_by_inviter(
                    f.workspace,
                    target,
                    &[WorkspaceRole::Member, WorkspaceRole::Member]
                )
                .await
                .unwrap(),
                1
            );
        }
        tx.commit_with_cleanup().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM invitations WHERE id=?1")
                .bind(member.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        for id in [owner, accepted, other, other_tenant] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM invitations WHERE id=?1")
                    .bind(id.as_bytes().as_slice())
                    .fetch_one(&f.pool)
                    .await
                    .unwrap(),
                1
            );
        }
        // Only the invitation snapshot changes; no publication/other business rows.
        let after = snapshot(&f).await;
        assert_ne!(after[6], before[6]);
        for (index, (a, b)) in before.iter().zip(after.iter()).enumerate() {
            if index != 6 {
                assert_eq!(a, b);
            }
        }
        crate::db::workspace::selected_personal_workspace_tests::foreign_keys(&f).await;
        f.close().await;
    }
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

pub async fn remove_by_workspace(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM fvoci.invitations WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
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

#[allow(clippy::too_many_arguments)]
pub async fn create_invitation_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    email: &str,
    role: WorkspaceRole,
    ip: Option<&str>,
) -> Result<Result<CreatedInvitation, InvitationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_invitation(pool, ws, actor, credential, email, role, ip).await;
    }
    let mut tx = backend.begin_write().await?;
    let token = new_token();
    let id = Uuid::now_v7();
    let result= async {
        let mut op = tx.operation();
        op.set_tenant(ws).await?;
        if !op.recheck_session(actor, credential).await? {
            return Ok(Err(InvitationDbError::Forbidden));
        }
        let kind = {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            f.query("SELECT kind FROM workspaces WHERE id=?1 AND deleted_at IS NULL", &[Cell::uuid(ws)]).await?.first().map(|r| r.cell(0)?.string()).transpose()?
        };
        let Some(kind) = kind else {
            return Ok(Err(InvitationDbError::NotFound));
        };
        let inviter = match op.membership_role(ws, actor, true).await? {
            Some(r) if r.at_least(WorkspaceRole::Admin) => r,
            Some(_) => return Ok(Err(InvitationDbError::Forbidden)),
            None => return Ok(Err(InvitationDbError::NotFound)),
        };
        if kind == "personal" {
            return Ok(Err(InvitationDbError::PersonalImmutable));
        }
        if !inviter.at_least(role) {
            return Ok(Err(InvitationDbError::RoleCap));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            f.execute("INSERT INTO invitations(id,workspace_id,email,role,token_hash,invited_by,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7)", &[Cell::uuid(id), Cell::uuid(ws), Cell::text(email), Cell::text(role.as_str()), Cell::text(&token.hash), Cell::uuid(actor), Cell::instant(crate::db::identity::stored_now() + INVITE_TTL)?]).await?;
        }
        let payload = json!({
        "workspaceId":ws.to_string(),
        "email":email,
        "invitationId":id.to_string()
        });
        op.append_event(crate::db::identity::EventAppend { id: Uuid::now_v7(), workspace_id: Some(ws), actor_user_id: Some(actor), verb: "invitation.created".into(), target_type: Some("invitation".into()), target_id: Some(id), payload: payload.clone() }).await?;
        op.append_audit(crate::db::identity::AuditAppend { id: Uuid::now_v7(), workspace_id: Some(ws), actor_user_id: Some(actor), verb: "invitation.created".into(), target_type: Some("invitation".into()), target_id: Some(id), payload, ip: ip.map(str::to_string) }).await?;
        Ok(Ok(CreatedInvitation { accept_path_token: token.token }))
    }.await;
    finish_invitation(tx, result, true).await
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
    let required_legal = crate::db::legal::required_latest(pool)
        .await?
        .into_iter()
        .map(|doc| (doc.kind, doc.version, doc.title))
        .collect();
    Ok(Ok(InvitationPublic {
        workspace_name: name,
        email_masked: mask_email(&invitation.email),
        role: invitation.role,
        required_legal,
    }))
}

pub async fn get_invitation_public_backend(
    backend: &Backend,
    raw: &str,
) -> Result<Result<InvitationPublic, InvitationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_invitation_public(pool, raw).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let invite = match family_invitation(&mut op, &hash_token(raw)).await? {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        op.set_tenant(invite.workspace_id).await?;
        let name = {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            f.query("SELECT name FROM workspaces WHERE id=?1 AND deleted_at IS NULL AND kind<>'personal'", &[Cell::uuid(invite.workspace_id)]).await?.first().map(|r| r.cell(0)?.string()).transpose()?
        };
        let Some(workspace_name) = name else {
            return Ok(Err(InvitationDbError::NotFound));
        };
        if !family_inviter_authorized(&mut op, &invite).await? {
            return Ok(Err(InvitationDbError::NotFound));
        }
        let required_legal = family_invitation_legal(&mut op).await?;
        Ok(Ok(InvitationPublic { workspace_name, email_masked: mask_email(&invite.email), role: invite.role, required_legal }))
    }.await;
    finish_invitation(tx, result, false).await
}

/// `password_gate` runs with the invited address right before the password
/// of an existing account is checked (the route charges login's budget);
/// `Err(retry_after)` refuses the attempt without checking.
pub async fn accept_invitation<G, F>(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    keys: &Keyring,
    raw_token: &str,
    request: AcceptInvitationRequest<'_>,
    password_gate: G,
) -> Result<Result<crate::db::mfa::Issued, InvitationDbError>, sqlx::Error>
where
    G: FnOnce(String) -> F,
    F: std::future::Future<Output = Result<(), u32>>,
{
    let invitation = match load_invitation_by_token(pool, raw_token).await? {
        Ok(row) => row,
        Err(err) => return Ok(Err(err)),
    };
    if let Some(email) = request.email {
        if email != invitation.email {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
    }

    let required = crate::db::legal::required_latest(pool).await?;
    if !crate::db::legal::covers_required(&required, request.consents) {
        return Ok(Err(InvitationDbError::ConsentRequired));
    }
    let existing = find_user_id_by_email(pool, &invitation.email).await?;
    let mut new_account = None;
    let user_id;
    let mut rehash = None;

    if let Some((existing_id, suspended_at)) = existing {
        if let Err(retry_after) = password_gate(invitation.email.clone()).await {
            return Ok(Err(InvitationDbError::RateLimited(retry_after)));
        }
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
            password_hash: Some(password_hash),
            defaults: request.defaults.clone(),
        });
    }

    if let Err(err) = grant_membership(
        pool,
        license,
        &invitation,
        user_id,
        GrantDetails {
            new_account,
            link: None,
            client_ip: request.client_ip,
            consents: request.consents,
        },
    )
    .await?
    {
        return Ok(Err(err));
    }
    if let Some((expected, replacement)) = rehash {
        let _ = rehash_password_if_unchanged(pool, user_id, &replacement, &expected).await;
    }
    match crate::db::mfa::issue_session_or_challenge(
        pool,
        user_id,
        "invitation",
        crate::db::mfa::IssueOptions::default(),
    )
    .await?
    {
        Some(issued) => Ok(Ok(issued)),
        None => Ok(Err(InvitationDbError::Unauthorized)),
    }
}

pub async fn accept_invitation_backend<G, F>(
    backend: &Backend,
    license: &crate::license::Entitlements,
    keys: &Keyring,
    raw: &str,
    request: AcceptInvitationRequest<'_>,
    password_gate: G,
) -> Result<Result<crate::db::mfa::Issued, InvitationDbError>, sqlx::Error>
where
    G: FnOnce(String) -> F,
    F: std::future::Future<Output = Result<(), u32>>,
{
    if let Backend::Postgres(pool) = backend {
        return accept_invitation(pool, license, keys, raw, request, password_gate).await;
    }
    let hash = hash_token(raw);
    let mut read = backend.begin_read().await?;
    let preflight= async {
        let mut op = read.operation();
        let invite = match family_invitation(&mut op, &hash).await? {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        if request.email.is_some_and(|e| e != invite.email) {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        let required = family_invitation_legal(&mut op).await?;
        if !required.iter().all(|(k, v, _)| request.consents.iter().any(|(ck, cv)| k == ck && v == cv)) {
            return Ok(Err(InvitationDbError::ConsentRequired));
        }
        let previous = op.set_system().await?;
        let account = async {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            let rows = f.query("SELECT id,password_hash,deleted_at IS NULL AND suspended_at IS NULL FROM users WHERE email=?1", &[Cell::text(&invite.email)]).await?;
            rows.first().map(|r| Ok::<_, sqlx::Error>((r.cell(0)?.id()?, r.cell(1)?.optional(Cell::string)?, r.cell(2)?.boolean()?))).transpose()
        }
        .await;
        op.restore_system(previous).await?;
        Ok(Ok((invite, account?)))
    }.await;
    let (invite, account) = match finish_invitation(read, preflight, false).await? {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let mut rehash = None;
    let (user, new_account, expected_hash) = if let Some((user, stored, live)) = account {
        if let Err(n) = password_gate(invite.email.clone()).await {
            return Ok(Err(InvitationDbError::RateLimited(n)));
        }
        let verified =
            verify_password(stored.as_deref(), request.password.unwrap_or(""), keys).await;
        if !verified.ok || stored.is_none() || !live {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        if verified.needs_pepper_rotation {
            if let Ok(replacement) = hash_password(request.password.unwrap_or(""), keys).await {
                rehash = stored.clone().map(|expected| (expected, replacement));
            }
        }
        (user, None, stored)
    } else {
        let (Some(email), Some(given_name), Some(password)) =
            (request.email, request.given_name, request.password)
        else {
            return Ok(Err(InvitationDbError::Unauthorized));
        };
        let password_hash = match hash_password(password, keys).await {
            Ok(h) => h,
            Err(_) => return Ok(Err(InvitationDbError::Unauthorized)),
        };
        (
            Uuid::now_v7(),
            Some(NewAccount {
                email: email.into(),
                given_name: given_name.into(),
                family_name: request
                    .family_name
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                password_hash: Some(password_hash),
                defaults: request.defaults.clone(),
            }),
            None,
        )
    };
    if let Err(e) = grant_family_invitation(
        backend,
        license,
        &invite,
        user,
        FamilyGrantDetails {
            new_account,
            expected_password: expected_hash,
            identity: None,
            identity_existing: false,
            client_ip: request.client_ip,
            consents: request.consents,
        },
    )
    .await?
    {
        return Ok(Err(e));
    }
    if let Some((expected, replacement)) = rehash {
        // Match PostgreSQL's optional CAS rotation after a committed grant.
        let _ = rehash_invited_family_password(backend, user, &replacement, &expected).await;
    }
    Ok(crate::db::mfa::issue_session_or_challenge_backend(
        backend,
        user,
        "invitation",
        crate::db::mfa::IssueOptions::default(),
    )
    .await?
    .ok_or(InvitationDbError::Unauthorized))
}

/// Source `acceptInviteWithIdentity` after the provider exchange.
pub struct IdentityAcceptRequest<'a> {
    pub token_hash: &'a str,
    pub provider: &'a str,
    /// Provider subject (invite links never carry a workspace prefix).
    pub subject: &'a str,
    /// Issuer that verified `subject` (see `db::oidc::find_link`).
    pub issuer: &'a str,
    pub link_email: Option<&'a str>,
    pub given_name: Option<&'a str>,
    pub client_ip: Option<&'a str>,
    pub consents: &'a [(String, i32)],
    pub defaults: &'a crate::settings::DefaultsUserSettings,
}

/// Grants the invitation to the account behind an external identity. An
/// existing account must already own that identity; otherwise a new
/// password-less account is created with the link. Returns the user id; the
/// caller issues the session through the MFA gate.
pub async fn accept_invitation_with_identity(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    request: IdentityAcceptRequest<'_>,
) -> Result<Result<Uuid, InvitationDbError>, sqlx::Error> {
    let invitation = match load_invitation_by_hash(pool, request.token_hash).await? {
        Ok(row) => row,
        Err(err) => return Ok(Err(err)),
    };
    let required = crate::db::legal::required_latest(pool).await?;
    if !crate::db::legal::covers_required(&required, request.consents) {
        return Ok(Err(InvitationDbError::ConsentRequired));
    }
    let existing = find_user_id_by_email(pool, &invitation.email).await?;
    let link =
        crate::db::oidc::find_link(pool, request.provider, request.subject, request.issuer).await?;
    if let Some((user_id, suspended_at)) = existing {
        let owner = match &link {
            crate::db::oidc::LinkLookup::Found(l) => Some(l.user_id),
            _ => None,
        };
        if owner != Some(user_id) || suspended_at.is_some() {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        return Ok(grant_membership(
            pool,
            license,
            &invitation,
            user_id,
            GrantDetails {
                new_account: None,
                link: None,
                client_ip: request.client_ip,
                consents: request.consents,
            },
        )
        .await?
        .map(|()| user_id));
    }
    if email_exists(pool, &invitation.email).await? {
        return Ok(Err(InvitationDbError::Unauthorized));
    }
    if link.subject_taken() {
        return Ok(Err(InvitationDbError::AlreadyLinked));
    }
    let user_id = Uuid::now_v7();
    let given_name = request
        .given_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(100).collect::<String>())
        .unwrap_or_else(|| email_local_part(&invitation.email));
    let new_account = NewAccount {
        email: invitation.email.clone(),
        given_name,
        family_name: None,
        password_hash: None,
        defaults: request.defaults.clone(),
    };
    let link = crate::db::oidc::NewLink {
        user_id,
        provider: request.provider,
        subject: request.subject,
        issuer: request.issuer,
        email: request.link_email,
        workspace_id: None,
    };
    Ok(grant_membership(
        pool,
        license,
        &invitation,
        user_id,
        GrantDetails {
            new_account: Some(new_account),
            link: Some(&link),
            client_ip: request.client_ip,
            consents: request.consents,
        },
    )
    .await?
    .map(|()| user_id))
}

pub async fn accept_invitation_with_identity_backend(
    backend: &Backend,
    license: &crate::license::Entitlements,
    request: IdentityAcceptRequest<'_>,
) -> Result<Result<Uuid, InvitationDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return accept_invitation_with_identity(pool, license, request).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let invitation = match family_invitation(&mut op, request.token_hash).await? {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        let required = family_invitation_legal(&mut op).await?;
        if !required.iter().all(|(k, v, _)| request.consents.iter().any(|(ck, cv)| k == ck && v == cv)) {
            return Ok(Err(InvitationDbError::ConsentRequired));
        }
        let previous = op.set_system().await?;
        let lookup = async {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            let users = f.query("SELECT id,deleted_at IS NULL AND suspended_at IS NULL FROM users WHERE email=?1", &[Cell::text(&invitation.email)]).await?;
            let account = users.first().map(|r| Ok::<_, sqlx::Error>((r.cell(0)?.id()?, r.cell(1)?.boolean()?))).transpose()?;
            let links = f.query("SELECT user_id,issuer FROM identity_links WHERE provider=?1 AND provider_user_id=?2", &[Cell::text(request.provider), Cell::text(request.subject)]).await?;
            let link = links.first().map(|r| Ok::<_, sqlx::Error>((r.cell(0)?.id()?, r.cell(1)?.optional(Cell::string)?))).transpose()?;
            Ok::<_, sqlx::Error>((account, link))
        }
        .await;
        op.restore_system(previous).await?;
        let (account, link) = lookup?;
        Ok(Ok((invitation, account, link)))
    }.await;
    let (invite, account, existing_link) = match finish_invitation(tx, result, false).await? {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let (user, new_account, identity_existing) = if let Some((user, live)) = account {
        if !live
            || !existing_link.as_ref().is_some_and(|(owner, issuer)| {
                *owner == user && issuer.as_deref() == Some(request.issuer)
            })
        {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        (user, None, true)
    } else {
        if existing_link.is_some() {
            return Ok(Err(InvitationDbError::AlreadyLinked));
        }
        let given_name = request
            .given_name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.chars().take(100).collect())
            .unwrap_or_else(|| email_local_part(&invite.email));
        (
            Uuid::now_v7(),
            Some(NewAccount {
                email: invite.email.clone(),
                given_name,
                family_name: None,
                password_hash: None,
                defaults: request.defaults.clone(),
            }),
            false,
        )
    };
    let link = crate::db::oidc::NewLink {
        user_id: user,
        provider: request.provider,
        subject: request.subject,
        issuer: request.issuer,
        email: request.link_email,
        workspace_id: None,
    };
    Ok(grant_family_invitation(
        backend,
        license,
        &invite,
        user,
        FamilyGrantDetails {
            new_account,
            expected_password: None,
            identity: Some(&link),
            identity_existing,
            client_ip: request.client_ip,
            consents: request.consents,
        },
    )
    .await?
    .map(|()| user))
}

/// Source `emailLocalPart`: the display name fallback for a new account.
pub fn email_local_part(email: &str) -> String {
    email.split('@').next().unwrap_or(email).to_string()
}

async fn grant_membership(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    invitation: &InvitationRow,
    user_id: Uuid,
    details: GrantDetails<'_>,
) -> Result<Result<(), InvitationDbError>, sqlx::Error> {
    let GrantDetails {
        new_account,
        link,
        client_ip,
        consents,
    } = details;
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
            INSERT INTO fvoci.users (
                id, email, password_hash, given_name, family_name,
                locale, timezone, week_starts_on, text_scale
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(user_id)
        .bind(&new_account.email)
        .bind(&new_account.password_hash)
        .bind(&new_account.given_name)
        .bind(&new_account.family_name)
        .bind(&new_account.defaults.locale)
        .bind(&new_account.defaults.timezone)
        .bind(new_account.defaults.week_starts_on as i32)
        .bind(new_account.defaults.text_scale as i16)
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
        if let Err(err) =
            require_membership_admission(&mut tx, user_id, current.role, None, license).await?
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
    // Source acceptInviteWithIdentity: the new account's link commits with
    // its membership.
    if let Some(link) = link {
        if !crate::db::oidc::insert_link(&mut tx, link).await? {
            tx.rollback().await?;
            return Ok(Err(InvitationDbError::AlreadyLinked));
        }
    }
    // Signup consents commit with the membership (source grantMembership).
    if !consents.is_empty() {
        set_self_user(&mut tx, user_id).await?;
        crate::db::legal::insert_consents(
            &mut tx,
            user_id,
            consents,
            client_ip,
            crate::db::legal::ConsentChannel::Signup,
        )
        .await?;
        clear_self_user(&mut tx).await?;
    }
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
    load_invitation_by_hash(pool, &hash_token(raw_token)).await
}

async fn load_invitation_by_hash(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Result<InvitationRow, InvitationDbError>, sqlx::Error> {
    let token_hash = token_hash.to_string();
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

#[derive(Debug, thiserror::Error)]
#[error("invitation operation refused: {0:?}")]
struct InvitationRefusal(InvitationDbError);
async fn finish_invitation<T>(
    tx: DbTx,
    result: Result<Result<T, InvitationDbError>, sqlx::Error>,
    write: bool,
) -> Result<Result<T, InvitationDbError>, sqlx::Error> {
    if write && matches!(&result, Ok(Ok(_))) {
        tx.commit_with_cleanup()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    } else if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Err(e) => Some(Box::new(e)),
            Ok(Err(e)) => Some(Box::new(InvitationRefusal(e))),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}
async fn family_invitation(
    op: &mut OperationTx<'_, '_>,
    hash: &str,
) -> Result<Result<InvitationRow, InvitationDbError>, sqlx::Error> {
    let previous = op.set_system().await?;
    let result= async {
        let OperationTx::SqliteFamily(f) = op else { unreachable!() };
        f.require_system_context()?;
        let rows = f.query("SELECT id,workspace_id,email,role,token_hash,invited_by,expires_at,accepted_at FROM invitations WHERE token_hash=?1", &[Cell::text(hash)]).await?;
        let Some(r) = rows.first() else {
            return Ok(Err(InvitationDbError::NotFound));
        };
        let row = InvitationRow { id: r.cell(0)?.id()?, workspace_id: r.cell(1)?.id()?, email: r.cell(2)?.string()?, role: WorkspaceRole::parse(&r.cell(3)?.string()?).ok_or_else(|| sqlx::Error::Protocol("invalid invitation role".into()))?, token_hash: r.cell(4)?.string()?, invited_by: r.cell(5)?.id()?, expires_at: r.cell(6)?.datetime()?, accepted_at: r.cell(7)?.optional(Cell::datetime)? };
        if row.accepted_at.is_some() {
            return Ok(Err(InvitationDbError::AlreadyAccepted));
        }
        if row.expires_at <= Utc::now() {
            return Ok(Err(InvitationDbError::Expired));
        }
        Ok(Ok(row))
    }.await;
    op.restore_system(previous).await?;
    result
}
async fn family_inviter_authorized(
    op: &mut OperationTx<'_, '_>,
    invite: &InvitationRow,
) -> Result<bool, sqlx::Error> {
    op.set_tenant(invite.workspace_id).await?;
    let live = {
        let OperationTx::SqliteFamily(f) = op else {
            unreachable!()
        };
        !f.query(
            "SELECT 1 FROM users WHERE id=?1 AND deleted_at IS NULL AND suspended_at IS NULL",
            &[Cell::uuid(invite.invited_by)],
        )
        .await?
        .is_empty()
    };
    Ok(live
        && op
            .membership_role(invite.workspace_id, invite.invited_by, false)
            .await?
            .is_some_and(|r| r.at_least(WorkspaceRole::Admin) && r.at_least(invite.role)))
}
async fn family_invitation_legal(
    op: &mut OperationTx<'_, '_>,
) -> Result<Vec<(String, i32, String)>, sqlx::Error> {
    let previous = op.set_system().await?;
    let result= async {
        let OperationTx::SqliteFamily(f) = op else { unreachable!() };
        f.query("SELECT d.kind,d.version,d.title FROM legal_documents d WHERE d.required=1 AND d.version=(SELECT max(x.version) FROM legal_documents x WHERE x.kind=d.kind) ORDER BY d.kind DESC", &[]).await?.iter().map(|r| Ok((r.cell(0)?.string()?, r.cell(1)?.int32()?, r.cell(2)?.string()?))).collect::<Result<Vec<_>, sqlx::Error>>()
    }.await;
    op.restore_system(previous).await?;
    result
}

struct FamilyGrantDetails<'a> {
    new_account: Option<NewAccount>,
    expected_password: Option<String>,
    identity: Option<&'a crate::db::oidc::NewLink<'a>>,
    identity_existing: bool,
    client_ip: Option<&'a str>,
    consents: &'a [(String, i32)],
}
async fn grant_family_invitation(
    backend: &Backend,
    license: &crate::license::Entitlements,
    invite: &InvitationRow,
    user: Uuid,
    details: FamilyGrantDetails<'_>,
) -> Result<Result<(), InvitationDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        op.acquire_admission_lock().await?;
        let current = match family_invitation(&mut op, &invite.token_hash).await? {
            Ok(i) => i,
            Err(e) => return Ok(Err(e)),
        };
        op.set_tenant(current.workspace_id).await?;
        if current.id != invite.id || current.email != invite.email {
            return Ok(Err(InvitationDbError::NotFound));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            if f.query("SELECT 1 FROM workspaces WHERE id=?1 AND deleted_at IS NULL AND kind<>'personal'", &[Cell::uuid(current.workspace_id)]).await?.is_empty() {
                return Ok(Err(InvitationDbError::NotFound));
            }
        }
        if !family_inviter_authorized(&mut op, &current).await? {
            return Ok(Err(InvitationDbError::Unauthorized));
        }
        let required = family_invitation_legal(&mut op).await?;
        if !required.iter().all(|(k, v, _)| details.consents.iter().any(|(ck, cv)| k == ck && v == cv)) {
            return Ok(Err(InvitationDbError::ConsentRequired));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            if let Some(new) = details.new_account {
                let n = f.execute("INSERT INTO users(id,email,password_hash,given_name,family_name,locale,timezone,week_starts_on,text_scale) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(email) DO NOTHING", &[Cell::uuid(user), Cell::text(new.email), Cell::optional_text(new.password_hash.as_deref()), Cell::text(new.given_name), Cell::optional_text(new.family_name.as_deref()), Cell::text(new.defaults.locale), Cell::text(new.defaults.timezone), Cell::Integer(new.defaults.week_starts_on), Cell::Integer(new.defaults.text_scale)]).await?;
                if n == 0 {
                    return Ok(Err(InvitationDbError::Unauthorized));
                }
            } else if f.query("SELECT 1 FROM users WHERE id=?1 AND email=?2 AND deleted_at IS NULL AND suspended_at IS NULL AND (?3 IS NULL OR password_hash=?3)", &[Cell::uuid(user), Cell::text(&current.email), Cell::optional_text(details.expected_password.as_deref())]).await?.is_empty() {
                return Ok(Err(InvitationDbError::Unauthorized));
            }
        }
        if op.membership_role(current.workspace_id, user, true).await?.is_none() {
            if current.role != WorkspaceRole::Guest {
                if let Err(e) = op.require_new_instance_billable_user(Some(user), license).await? {
                    return Ok(Err(quota_error(e)));
                }
            }
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            f.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)", &[Cell::uuid(current.workspace_id), Cell::uuid(user), Cell::text(current.role.as_str())]).await?;
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            f.execute("UPDATE invitations SET accepted_at=?2,updated_at=?2 WHERE id=?1 AND accepted_at IS NULL", &[Cell::uuid(current.id), Cell::instant(crate::db::identity::stored_now())?]).await?;
            for (kind, version) in details.consents {
                f.execute("INSERT INTO user_consents(id,user_id,kind,version,consented_at,ip,channel) SELECT ?1,?2,?3,?4,?5,?6,'signup' WHERE ?4=(SELECT max(version) FROM legal_documents WHERE kind=?3) ON CONFLICT(user_id,kind,version) DO NOTHING", &[Cell::uuid(Uuid::now_v7()), Cell::uuid(user), Cell::text(kind), Cell::Integer(i64::from(*version)), Cell::instant(crate::db::identity::stored_now())?, Cell::optional_text(details.client_ip)]).await?;
            }
        }
        let payload = json!({
        "workspaceId":current.workspace_id.to_string(),
        "userId":user.to_string(),
        "email":current.email
        });
        op.append_event(crate::db::identity::EventAppend { id: Uuid::now_v7(), workspace_id: Some(current.workspace_id), actor_user_id: Some(user), verb: "invitation.accepted".into(), target_type: Some("invitation".into()), target_id: Some(current.id), payload: payload.clone() }).await?;
        op.append_audit(crate::db::identity::AuditAppend { id: Uuid::now_v7(), workspace_id: Some(current.workspace_id), actor_user_id: Some(user), verb: "invitation.accepted".into(), target_type: Some("invitation".into()), target_id: Some(current.id), payload, ip: details.client_ip.map(str::to_string) }).await?;
        if let Some(link) = details.identity {
            let previous = op.set_system().await?;
            let identity_result = async {
                let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
                if details.identity_existing {
                    return Ok::<_, sqlx::Error>(!f.query("SELECT 1 FROM identity_links WHERE user_id=?1 AND provider=?2 AND provider_user_id=?3 AND issuer=?4", &[Cell::uuid(user), Cell::text(link.provider), Cell::text(link.subject), Cell::text(link.issuer)]).await?.is_empty());
                }
                let id = Uuid::now_v7();
                let n = f.execute("INSERT INTO identity_links(id,user_id,provider,provider_user_id,email,issuer) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING", &[Cell::uuid(id), Cell::uuid(user), Cell::text(link.provider), Cell::text(link.subject), Cell::optional_text(link.email), Cell::text(link.issuer)]).await?;
                if n == 0 {
                    return Ok(false);
                }
                op.append_event(crate::db::identity::EventAppend {
                    id: Uuid::now_v7(),
                    workspace_id: None,
                    actor_user_id: Some(user),
                    verb: "identity.linked".into(),
                    target_type: Some("identity_link".into()),
                    target_id: Some(id),
                    payload: json!({
                    "userId":user.to_string(),
                    "provider":link.provider,
                    "email":link.email
                    }),
                })
                .await?;
                Ok(true)
            }
            .await;
            op.restore_system(previous).await?;
            if !identity_result? {
                return Ok(Err(InvitationDbError::AlreadyLinked));
            }
        }
        Ok(Ok(()))
    }.await;

    finish_invitation(tx, result, true).await
}

async fn rehash_invited_family_password(
    backend: &Backend,
    user: Uuid,
    replacement: &str,
    expected: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_system().await?;
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        f.execute("UPDATE users SET password_hash=?2,updated_at=?4 WHERE id=?1 AND deleted_at IS NULL AND password_hash=?3", &[Cell::uuid(user), Cell::text(replacement), Cell::text(expected), Cell::instant(crate::db::identity::stored_now())?]).await?;
        Ok::<_, sqlx::Error>(Ok::<_, InvitationDbError>(()))
    }.await;
    finish_invitation(tx, result, true)
        .await?
        .map_err(|_| sqlx::Error::Protocol("password rotation refused".into()))
}
