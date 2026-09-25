//! Identity links, workspace SSO configuration, OIDC flow state and JIT join
//! (source `packages/core/src/{oidc,workspace-oidc}.ts`, repos
//! `identityLinks` / `workspaceOidc`).

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    clear_self_user, lock_membership_users, recheck_session, restore_system, set_self_user,
    set_system, set_tenant,
};
use crate::db::identity::{append_event, lock_sign_in, EventAppend};
use crate::db::quota::{acquire_admission_lock, require_membership_admission};
use crate::db::workspace::WorkspaceRole;

#[derive(Debug, Clone)]
pub struct LinkRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

type LinkTuple = (Uuid, Uuid, String, Option<String>, DateTime<Utc>);

fn link_from(t: LinkTuple) -> LinkRow {
    LinkRow {
        id: t.0,
        user_id: t.1,
        provider: t.2,
        email: t.3,
        created_at: t.4,
    }
}

/// Sign-in lookup: no user is known yet, so it runs in system context.
pub async fn find_link(
    pool: &PgPool,
    provider: &str,
    subject: &str,
) -> Result<Option<LinkRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let row: Option<LinkTuple> = sqlx::query_as(
        r#"
        SELECT id, user_id, provider, email, created_at
        FROM fvoci.identity_links
        WHERE provider = $1 AND provider_user_id = $2
        "#,
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(link_from))
}

async fn links_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<LinkRow>, sqlx::Error> {
    set_self_user(tx, user_id).await?;
    let rows: Vec<LinkTuple> = sqlx::query_as(
        r#"
        SELECT id, user_id, provider, email, created_at
        FROM fvoci.identity_links
        WHERE user_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    clear_self_user(tx).await?;
    Ok(rows.into_iter().map(link_from).collect())
}

pub async fn list_links(pool: &PgPool, user_id: Uuid) -> Result<Vec<LinkRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let rows = links_in_tx(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(rows)
}

pub struct NewLink<'a> {
    pub user_id: Uuid,
    pub provider: &'a str,
    pub subject: &'a str,
    pub email: Option<&'a str>,
    pub workspace_id: Option<Uuid>,
}

/// Inserts the link and its `identity.linked` event in the caller's
/// transaction. False when the subject or the (user, provider) pair is taken.
pub(crate) async fn insert_link(
    tx: &mut Transaction<'_, Postgres>,
    link: &NewLink<'_>,
) -> Result<bool, sqlx::Error> {
    let previous = set_system(tx).await?;
    let id = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, email)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(link.user_id)
    .bind(link.provider)
    .bind(link.subject)
    .bind(link.email)
    .fetch_optional(&mut **tx)
    .await?;
    if inserted.is_some() {
        append_event(
            tx,
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: link.workspace_id,
                actor_user_id: Some(link.user_id),
                verb: "identity.linked".to_string(),
                target_type: Some("identity_link".to_string()),
                target_id: Some(id),
                payload: json!({
                    "userId": link.user_id.to_string(),
                    "provider": link.provider,
                    "email": link.email,
                }),
            },
        )
        .await?;
    }
    restore_system(tx, &previous).await?;
    Ok(inserted.is_some())
}

#[derive(Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Linked,
    AlreadyLinked,
    SessionGone,
}

/// Source `linkIdentity`: the subject must be free and the user must not
/// already have a link for this provider.
pub async fn link_for_user(pool: &PgPool, link: &NewLink<'_>) -> Result<LinkOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, link.user_id).await?;
    let live: Option<(bool,)> = sqlx::query_as(
        "SELECT deleted_at IS NULL AND suspended_at IS NULL FROM fvoci.users WHERE id = $1",
    )
    .bind(link.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if live != Some((true,)) {
        tx.rollback().await?;
        return Ok(LinkOutcome::SessionGone);
    }
    let mine = links_in_tx(&mut tx, link.user_id).await?;
    if mine.iter().any(|l| l.provider == link.provider) || !insert_link(&mut tx, link).await? {
        tx.rollback().await?;
        return Ok(LinkOutcome::AlreadyLinked);
    }
    tx.commit().await?;
    Ok(LinkOutcome::Linked)
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnlinkOutcome {
    Ok,
    NotFound,
    LastMethod,
    SessionGone,
}

/// Source `unlinkIdentity` + `hasOtherSignInMethod`: a password, a verified
/// mailbox when mail sign-in is on, or another link must remain.
pub async fn unlink(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    provider: &str,
    mail_enabled: bool,
) -> Result<UnlinkOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, user_id).await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::SessionGone);
    }
    let links = links_in_tx(&mut tx, user_id).await?;
    let Some(link) = links.iter().find(|l| l.provider == provider).cloned() else {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::NotFound);
    };
    let has_password: Option<(bool,)> =
        sqlx::query_as("SELECT fvoci.app_user_password_hash($1) IS NOT NULL")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let verified: Option<(bool,)> =
        sqlx::query_as("SELECT email_verified_at IS NOT NULL FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let other = has_password == Some((true,))
        || (mail_enabled && verified == Some((true,)))
        || links.iter().any(|l| l.provider != provider);
    if !other {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::LastMethod);
    }
    set_self_user(&mut tx, user_id).await?;
    sqlx::query("DELETE FROM fvoci.identity_links WHERE user_id = $1 AND provider = $2")
        .bind(user_id)
        .bind(provider)
        .execute(&mut *tx)
        .await?;
    clear_self_user(&mut tx).await?;
    let previous = set_system(&mut tx).await?;
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(user_id),
            verb: "identity.unlinked".to_string(),
            target_type: Some("identity_link".to_string()),
            target_id: Some(link.id),
            payload: json!({
                "userId": user_id.to_string(),
                "provider": provider,
                "email": link.email,
            }),
        },
    )
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(UnlinkOutcome::Ok)
}

// ---------------------------------------------------------------------------
// Workspace SSO configuration

#[derive(Debug, Clone)]
pub struct WorkspaceOidcRow {
    pub workspace_id: Uuid,
    pub issuer: String,
    pub client_id: String,
    /// Sealed (`workspace-oidc:<workspace_id>`).
    pub client_secret: String,
    pub label: String,
}

type WorkspaceOidcTuple = (Uuid, String, String, String, String);

async fn workspace_oidc_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<WorkspaceOidcRow>, sqlx::Error> {
    let row: Option<WorkspaceOidcTuple> = sqlx::query_as(
        r#"
        SELECT workspace_id, issuer, client_id, client_secret, label
        FROM fvoci.workspace_oidc
        WHERE workspace_id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|t| WorkspaceOidcRow {
        workspace_id: t.0,
        issuer: t.1,
        client_id: t.2,
        client_secret: t.3,
        label: t.4,
    }))
}

/// Sign-in side: the row of a live workspace, tenant-scoped.
pub async fn workspace_oidc_for_sign_in(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<WorkspaceOidcRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let live: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let row = if live == Some((true,)) {
        workspace_oidc_in_tx(&mut tx, workspace_id).await?
    } else {
        None
    };
    tx.commit().await?;
    Ok(row)
}

pub async fn sso_workspace_by_slug(pool: &PgPool, slug: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_workspace_sso_id_by_slug($1)")
        .bind(slug)
        .fetch_one(pool)
        .await
}

/// Source `anyWorkspaceOidcConfigured` (live workspaces only).
pub async fn any_workspace_oidc(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let any: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.workspace_oidc o
            JOIN fvoci.workspaces w ON w.id = o.workspace_id
            WHERE w.deleted_at IS NULL
        )
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(any)
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManageError {
    NotFound,
    Forbidden,
    SessionGone,
}

/// Source `requirePermission(workspace, "manage")` under the tenant context:
/// a non-member sees 404, a member without manage 403.
async fn require_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ManageError>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    lock_membership_users(tx, &[actor]).await?;
    if !recheck_session(tx, actor, session_id).await? {
        return Ok(Err(ManageError::SessionGone));
    }
    let live: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    if live != Some((true,)) {
        return Ok(Err(ManageError::NotFound));
    }
    let role: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match role.and_then(|(r,)| WorkspaceRole::parse(&r)) {
        None => Ok(Err(ManageError::NotFound)),
        Some(role) if role.at_least(WorkspaceRole::Admin) => Ok(Ok(())),
        Some(_) => Ok(Err(ManageError::Forbidden)),
    }
}

pub async fn get_workspace_oidc(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
) -> Result<Result<Option<WorkspaceOidcRow>, ManageError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_manage(&mut tx, workspace_id, actor, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let row = workspace_oidc_in_tx(&mut tx, workspace_id).await?;
    tx.commit().await?;
    Ok(Ok(row))
}

pub struct WorkspaceOidcInput<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub sealed_secret: &'a str,
    pub label: &'a str,
}

pub async fn upsert_workspace_oidc(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
    input: WorkspaceOidcInput<'_>,
) -> Result<Result<WorkspaceOidcRow, ManageError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_manage(&mut tx, workspace_id, actor, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.workspace_oidc (id, workspace_id, issuer, client_id, client_secret, label)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (workspace_id) DO UPDATE
        SET issuer = EXCLUDED.issuer,
            client_id = EXCLUDED.client_id,
            client_secret = EXCLUDED.client_secret,
            label = EXCLUDED.label,
            updated_at = now()
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(input.issuer)
    .bind(input.client_id)
    .bind(input.sealed_secret)
    .bind(input.label)
    .execute(&mut *tx)
    .await?;
    let row = workspace_oidc_in_tx(&mut tx, workspace_id)
        .await?
        .expect("row after upsert");
    tx.commit().await?;
    Ok(Ok(row))
}

pub async fn remove_workspace_oidc(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ManageError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) = require_manage(&mut tx, workspace_id, actor, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let removed = sqlx::query("DELETE FROM fvoci.workspace_oidc WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if removed == 0 {
        tx.rollback().await?;
        return Ok(Err(ManageError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

// ---------------------------------------------------------------------------
// Flow state

pub async fn issue_state(
    pool: &PgPool,
    state_hash: &str,
    sealed_payload: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT fvoci.app_oidc_state_issue($1, $2, $3)")
        .bind(state_hash)
        .bind(sealed_payload)
        .bind(expires_at)
        .execute(pool)
        .await?;
    Ok(())
}

/// GETDEL: the state is gone after this call whatever the outcome.
pub async fn consume_state(pool: &PgPool, state_hash: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_oidc_state_consume($1)")
        .bind(state_hash)
        .fetch_one(pool)
        .await
}

// ---------------------------------------------------------------------------
// JIT join (workspace SSO with auto_join_domains)

pub async fn workspace_auto_join_domains(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(Vec<String>,)> = sqlx::query_as(
        "SELECT auto_join_domains FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|(d,)| d))
}

pub fn domain_allowed(domains: &[String], domain: &str) -> bool {
    domains
        .iter()
        .any(|entry| entry.trim().to_ascii_lowercase() == domain)
}

#[derive(Debug, PartialEq, Eq)]
pub enum JitOutcome {
    Joined(Uuid),
    Skipped,
    SeatLimit,
}

pub struct JitInput<'a> {
    pub workspace_id: Uuid,
    pub email: &'a str,
    pub domain: &'a str,
    pub given_name: &'a str,
    pub subject: &'a str,
}

/// Source `tryJitJoin` transaction: under the admission lock, recheck the
/// workspace, the domain and the free email, admit the seat, then create
/// the password-less account, its membership and its link together.
pub async fn jit_join(pool: &PgPool, input: JitInput<'_>) -> Result<JitOutcome, sqlx::Error> {
    let user_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    set_tenant(&mut tx, input.workspace_id).await?;
    lock_membership_users(&mut tx, &[user_id]).await?;
    let workspace: Option<(String, Option<DateTime<Utc>>, Vec<String>)> = sqlx::query_as(
        "SELECT kind, deleted_at, auto_join_domains FROM fvoci.workspaces WHERE id = $1 FOR UPDATE",
    )
    .bind(input.workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let eligible = matches!(
        &workspace,
        Some((kind, None, domains)) if kind != "personal" && domain_allowed(domains, input.domain)
    );
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM fvoci.users WHERE email = $1)")
            .bind(input.email)
            .fetch_one(&mut *tx)
            .await?;
    if !eligible || taken {
        tx.rollback().await?;
        return Ok(JitOutcome::Skipped);
    }
    if require_membership_admission(&mut tx, user_id, WorkspaceRole::Member, None)
        .await?
        .is_err()
    {
        tx.rollback().await?;
        return Ok(JitOutcome::SeatLimit);
    }
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, NULL, $3)",
    )
    .bind(user_id)
    .bind(input.email)
    .bind(input.given_name)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(input.workspace_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    let linked = insert_link(
        &mut tx,
        &NewLink {
            user_id,
            provider: "generic",
            subject: input.subject,
            email: Some(input.email),
            workspace_id: Some(input.workspace_id),
        },
    )
    .await?;
    if !linked {
        tx.rollback().await?;
        return Ok(JitOutcome::Skipped);
    }
    tx.commit().await?;
    Ok(JitOutcome::Joined(user_id))
}
