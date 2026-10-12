//! Identity links, workspace SSO configuration, OIDC flow state and JIT join
//! (source `packages/core/src/{oidc,workspace-oidc}.ts`, repos
//! `identityLinks` / `workspaceOidc`).

use super::backend::{Backend, OperationTx};
use super::codec::Cell;
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
type IssuedLinkTuple = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    DateTime<Utc>,
    Option<String>,
);

fn link_from(t: LinkTuple) -> LinkRow {
    LinkRow {
        id: t.0,
        user_id: t.1,
        provider: t.2,
        email: t.3,
        created_at: t.4,
    }
}

/// Result of looking a provider subject up for sign-in.
#[derive(Debug)]
pub enum LinkLookup {
    /// No link holds this subject.
    Missing,
    /// A link holds this subject but was verified by another issuer (the
    /// provider was reconfigured and the new IdP reuses the `sub`): it is not
    /// this identity, and its subject stays taken.
    OtherIssuer,
    Found(LinkRow),
}

impl LinkLookup {
    pub fn found(self) -> Option<LinkRow> {
        match self {
            Self::Found(link) => Some(link),
            Self::Missing | Self::OtherIssuer => None,
        }
    }

    pub fn subject_taken(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// Sign-in lookup: no user is known yet, so it runs in system context. A
/// legacy link without a verified issuer (including a Microsoft discovery
/// template) cannot establish which identity owns this subject. Keep it
/// occupied, but require an authenticated unlink and fresh link to use it.
pub async fn find_link(
    pool: &PgPool,
    provider: &str,
    subject: &str,
    issuer: &str,
) -> Result<LinkLookup, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let row: Option<IssuedLinkTuple> = sqlx::query_as(
        r#"
        SELECT id, user_id, provider, email, created_at, issuer
        FROM fvoci.identity_links
        WHERE provider = $1 AND provider_user_id = $2
        "#,
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, user_id, provider, email, created_at, stored_issuer)) = row else {
        tx.commit().await?;
        return Ok(LinkLookup::Missing);
    };
    let matches = stored_issuer.as_deref() == Some(issuer);
    tx.commit().await?;
    if !matches {
        // No subject, issuer or email values are logged.
        tracing::warn!(provider = %provider, "oidc.link_issuer_mismatch");
        return Ok(LinkLookup::OtherIssuer);
    }
    Ok(LinkLookup::Found(link_from((
        id, user_id, provider, email, created_at,
    ))))
}

pub async fn find_link_backend(
    backend: &Backend,
    provider: &str,
    subject: &str,
    issuer: &str,
) -> Result<LinkLookup, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return find_link(pool, provider, subject, issuer).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family OIDC lookup")
    };
    family.require_system_context()?;
    let rows=family.query("SELECT id,user_id,provider,email,created_at,issuer FROM identity_links WHERE provider=?1 AND provider_user_id=?2",&[Cell::text(provider),Cell::text(subject)]).await?;
    let result = match rows.first() {
        None => LinkLookup::Missing,
        Some(r) if r.cell(5)?.optional(Cell::string)?.as_deref() != Some(issuer) => {
            LinkLookup::OtherIssuer
        }
        Some(r) => LinkLookup::Found(family_link_row(r)?),
    };
    tx.rollback().await?;
    Ok(result)
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

pub async fn list_links_backend(
    backend: &Backend,
    user_id: Uuid,
) -> Result<Vec<LinkRow>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_links(pool, user_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let rows = family_links(&mut tx.operation(), user_id).await?;
    tx.rollback().await?;
    Ok(rows)
}

pub struct NewLink<'a> {
    pub user_id: Uuid,
    pub provider: &'a str,
    pub subject: &'a str,
    /// The issuer that verified the subject (discovery issuer, or the
    /// configured base URL of an OAuth2 provider without discovery).
    pub issuer: &'a str,
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
        INSERT INTO fvoci.identity_links (id, user_id, provider, provider_user_id, email, issuer)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(link.user_id)
    .bind(link.provider)
    .bind(link.subject)
    .bind(link.email)
    .bind(link.issuer)
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
/// already have a link for this provider. The session that started the flow
/// is rechecked under the sign-in lock, so a logout or revocation that
/// committed while the provider round-trip was in flight wins (the source
/// only compares the user id captured when the callback started).
pub async fn link_for_user(
    pool: &PgPool,
    session_id: Uuid,
    link: &NewLink<'_>,
) -> Result<LinkOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, link.user_id).await?;
    if !recheck_session(&mut tx, link.user_id, session_id).await? {
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

pub async fn link_for_user_backend(
    backend: &Backend,
    session_id: Uuid,
    link: &NewLink<'_>,
) -> Result<LinkOutcome, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return link_for_user(pool, session_id, link).await;
    }
    let mut tx = backend.begin_write().await?;
    if !tx
        .operation()
        .auth_session_live(link.user_id, session_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(LinkOutcome::SessionGone);
    }
    if family_links(&mut tx.operation(), link.user_id)
        .await?
        .iter()
        .any(|l| l.provider == link.provider)
        || !family_insert_link(&mut tx.operation(), link).await?
    {
        tx.rollback().await?;
        return Ok(LinkOutcome::AlreadyLinked);
    }
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
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

pub async fn unlink_backend(
    backend: &Backend,
    user_id: Uuid,
    session_id: Uuid,
    provider: &str,
    mail_enabled: bool,
) -> Result<UnlinkOutcome, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return unlink(pool, user_id, session_id, provider, mail_enabled).await;
    }
    let mut tx = backend.begin_write().await?;
    if !tx
        .operation()
        .auth_session_live(user_id, session_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::SessionGone);
    }
    let links = family_links(&mut tx.operation(), user_id).await?;
    let Some(link) = links.iter().find(|l| l.provider == provider) else {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::NotFound);
    };
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family OIDC unlink")
    };
    let rows = family
        .query(
            "SELECT password_hash IS NOT NULL,email_verified_at IS NOT NULL FROM users WHERE id=?1",
            &[Cell::uuid(user_id)],
        )
        .await?;
    let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
    if !row.cell(0)?.boolean()?
        && !(mail_enabled && row.cell(1)?.boolean()?)
        && !links.iter().any(|l| l.provider != provider)
    {
        tx.rollback().await?;
        return Ok(UnlinkOutcome::LastMethod);
    }
    family
        .execute(
            "DELETE FROM identity_links WHERE user_id=?1 AND provider=?2",
            &[Cell::uuid(user_id), Cell::text(provider)],
        )
        .await?;
    let mut op = tx.operation();
    op.set_system().await?;
    op.append_event(EventAppend {
        id: Uuid::now_v7(),
        workspace_id: None,
        actor_user_id: Some(user_id),
        verb: "identity.unlinked".into(),
        target_type: Some("identity_link".into()),
        target_id: Some(link.id),
        payload: json!({"userId":user_id.to_string(),"provider":provider,"email":link.email}),
    })
    .await?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
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

/// Sign-in side: the row of a live team workspace, tenant-scoped. A personal
/// workspace never signs anyone in: any user can create one, so its IdP
/// would be one they chose. A row saved there before that rule stays
/// readable and removable by its owner but starts no flow and completes none.
pub async fn workspace_oidc_for_sign_in(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Option<WorkspaceOidcRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let live: Option<(bool,)> = sqlx::query_as(
        "SELECT deleted_at IS NULL AND kind <> 'personal' FROM fvoci.workspaces WHERE id = $1",
    )
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

pub async fn workspace_oidc_for_sign_in_backend(
    backend: &Backend,
    workspace_id: Uuid,
) -> Result<Option<WorkspaceOidcRow>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return workspace_oidc_for_sign_in(pool, workspace_id).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace_id).await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family SSO lookup")
    };
    let rows=family.query("SELECT o.workspace_id,o.issuer,o.client_id,o.client_secret,o.label FROM workspace_oidc o JOIN workspaces w ON w.id=o.workspace_id WHERE w.id=?1 AND w.deleted_at IS NULL AND w.kind<>'personal'",&[Cell::uuid(workspace_id)]).await?;
    let row = rows.first().map(family_workspace_oidc_row).transpose()?;
    tx.rollback().await?;
    Ok(row)
}

pub async fn sso_workspace_by_slug(pool: &PgPool, slug: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_workspace_sso_id_by_slug($1)")
        .bind(slug)
        .fetch_one(pool)
        .await
}

pub async fn sso_workspace_by_slug_backend(
    backend: &Backend,
    slug: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return sso_workspace_by_slug(pool, slug).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family SSO slug")
    };
    family.require_system_context()?;
    let rows=family.query("SELECT w.id FROM workspaces w JOIN workspace_oidc o ON o.workspace_id=w.id WHERE w.slug=?1 AND w.deleted_at IS NULL",&[Cell::text(slug)]).await?;
    let row = rows.first().map(|r| r.cell(0)?.id()).transpose()?;
    tx.rollback().await?;
    Ok(row)
}

/// Source `anyWorkspaceOidcConfigured` (live team workspaces only: a
/// personal workspace's row signs no one in).
pub async fn any_workspace_oidc(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let any: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.workspace_oidc o
            JOIN fvoci.workspaces w ON w.id = o.workspace_id
            WHERE w.deleted_at IS NULL AND w.kind <> 'personal'
        )
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(any)
}

pub async fn any_workspace_oidc_backend(backend: &Backend) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return any_workspace_oidc(pool).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family SSO availability")
    };
    family.require_system_context()?;
    let rows=family.query("SELECT EXISTS(SELECT 1 FROM workspace_oidc o JOIN workspaces w ON w.id=o.workspace_id WHERE w.deleted_at IS NULL AND w.kind<>'personal')",&[]).await?;
    let any = rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .boolean()?;
    tx.rollback().await?;
    Ok(any)
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManageError {
    NotFound,
    Forbidden,
    SessionGone,
    /// Saving a configuration on a personal workspace.
    PersonalWorkspace,
}

/// The credential admitted by the route, retained for the writer's recheck.
pub enum ManageCredential {
    Session(Uuid),
    ApiToken { id: Uuid, hash: String },
}

impl ManageCredential {
    fn id(&self) -> Uuid {
        match self {
            Self::Session(id) | Self::ApiToken { id, .. } => *id,
        }
    }
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

pub async fn get_workspace_oidc_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: Uuid,
    credential: ManageCredential,
) -> Result<Result<Option<WorkspaceOidcRow>, ManageError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_workspace_oidc(pool, workspace_id, actor, credential.id()).await;
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) =
        family_require_manage(&mut tx.operation(), workspace_id, actor, &credential).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let row = family_workspace_oidc(&mut tx.operation(), workspace_id).await?;
    tx.rollback().await?;
    Ok(Ok(row))
}

pub struct WorkspaceOidcInput<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub sealed_secret: &'a str,
    pub label: &'a str,
}

/// Existing identity links are left alone: they keep the issuer that
/// verified them, so a new issuer's same `sub` does not reach old accounts.
///
/// A personal workspace takes no configuration: any user can create one and
/// would choose its IdP (see [`workspace_oidc_for_sign_in`]). Reading and
/// removing stay open so an owner can clear a row saved before this rule.
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
    let personal: bool =
        sqlx::query_scalar("SELECT kind = 'personal' FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?;
    if personal {
        tx.rollback().await?;
        return Ok(Err(ManageError::PersonalWorkspace));
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

pub async fn upsert_workspace_oidc_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: Uuid,
    credential: ManageCredential,
    input: WorkspaceOidcInput<'_>,
) -> Result<Result<WorkspaceOidcRow, ManageError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return upsert_workspace_oidc(pool, workspace_id, actor, credential.id(), input).await;
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) =
        family_require_manage(&mut tx.operation(), workspace_id, actor, &credential).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family SSO save")
    };
    family.require_writer()?;
    family.require_tenant(workspace_id)?;
    let rows = family
        .query(
            "SELECT kind FROM workspaces WHERE id=?1",
            &[Cell::uuid(workspace_id)],
        )
        .await?;
    if rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .string()?
        == "personal"
    {
        tx.rollback().await?;
        return Ok(Err(ManageError::PersonalWorkspace));
    }
    family.execute("INSERT INTO workspace_oidc (id,workspace_id,issuer,client_id,client_secret,label) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(workspace_id) DO UPDATE SET issuer=excluded.issuer,client_id=excluded.client_id,client_secret=excluded.client_secret,label=excluded.label,updated_at=?7",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace_id),Cell::text(input.issuer),Cell::text(input.client_id),Cell::text(input.sealed_secret),Cell::text(input.label),Cell::instant(crate::db::identity::stored_now())?]).await?;
    let row = family_workspace_oidc(&mut tx.operation(), workspace_id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
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

pub async fn remove_workspace_oidc_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: Uuid,
    credential: ManageCredential,
) -> Result<Result<(), ManageError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return remove_workspace_oidc(pool, workspace_id, actor, credential.id()).await;
    }
    let mut tx = backend.begin_write().await?;
    if let Err(e) =
        family_require_manage(&mut tx.operation(), workspace_id, actor, &credential).await?
    {
        tx.rollback().await?;
        return Ok(Err(e));
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family SSO remove")
    };
    family.require_writer()?;
    if family
        .execute(
            "DELETE FROM workspace_oidc WHERE workspace_id=?1",
            &[Cell::uuid(workspace_id)],
        )
        .await?
        != 1
    {
        tx.rollback().await?;
        return Ok(Err(ManageError::NotFound));
    }
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
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

pub async fn issue_state_backend(
    backend: &Backend,
    state_hash: &str,
    sealed_payload: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return issue_state(pool, state_hash, sealed_payload, expires_at).await;
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family OIDC state issue")
    };
    family.require_writer()?;
    family.require_system_context()?;
    family
        .execute(
            "INSERT INTO oidc_states (state_hash,payload,expires_at) VALUES (?1,?2,?3)",
            &[
                Cell::text(state_hash),
                Cell::text(sealed_payload),
                Cell::Integer(expires_at.timestamp_micros()),
            ],
        )
        .await?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))
}

/// GETDEL: the state is gone after this call whatever the outcome.
pub async fn consume_state(pool: &PgPool, state_hash: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_oidc_state_consume($1)")
        .bind(state_hash)
        .fetch_one(pool)
        .await
}

/// Family consumes only live states; expired rows remain for token GC.
pub async fn consume_state_backend(
    backend: &Backend,
    state_hash: &str,
) -> Result<Option<String>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return consume_state(pool, state_hash).await;
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family OIDC state consume")
    };
    family.require_writer()?;
    family.require_system_context()?;
    let rows = family
        .query(
            "DELETE FROM oidc_states WHERE state_hash=?1 AND expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) RETURNING payload",
            &[Cell::text(state_hash)],
        )
        .await?;
    let payload = rows.first().map(|r| r.cell(0)?.string()).transpose()?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
    Ok(payload)
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

pub async fn workspace_auto_join_domains_backend(
    backend: &Backend,
    workspace_id: Uuid,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return workspace_auto_join_domains(pool, workspace_id).await;
    }
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace_id).await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family auto-join domains")
    };
    let rows = family
        .query(
            "SELECT auto_join_domains FROM workspaces WHERE id=?1 AND deleted_at IS NULL",
            &[Cell::uuid(workspace_id)],
        )
        .await?;
    let domains = rows
        .first()
        .map(|r| {
            serde_json::from_value(r.cell(0)?.value()?)
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))
        })
        .transpose()?;
    tx.rollback().await?;
    Ok(domains)
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
    pub issuer: &'a str,
}

/// Source `tryJitJoin` transaction: under the admission lock, recheck the
/// workspace, the domain and the free email, admit the seat, then create
/// the password-less account, its membership and its link together.
pub async fn jit_join(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    input: JitInput<'_>,
) -> Result<JitOutcome, sqlx::Error> {
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
    if require_membership_admission(&mut tx, user_id, WorkspaceRole::Member, None, license)
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
            issuer: input.issuer,
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

pub async fn jit_join_backend(
    backend: &Backend,
    license: &crate::license::Entitlements,
    input: JitInput<'_>,
) -> Result<JitOutcome, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return jit_join(pool, license, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut op = tx.operation();
    op.set_tenant(input.workspace_id).await?;
    op.set_system().await?;
    let OperationTx::SqliteFamily(family) = op else {
        unreachable!("family OIDC JIT")
    };
    family.require_writer()?;
    family.require_tenant(input.workspace_id)?;
    family.require_system_context()?;
    let rows=family.query("SELECT auto_join_domains FROM workspaces WHERE id=?1 AND deleted_at IS NULL AND kind<>'personal'",&[Cell::uuid(input.workspace_id)]).await?;
    let domains: Option<Vec<String>> = rows
        .first()
        .map(|r| {
            serde_json::from_value(r.cell(0)?.value()?)
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))
        })
        .transpose()?;
    let taken = family
        .query(
            "SELECT EXISTS(SELECT 1 FROM users WHERE email=?1)",
            &[Cell::text(input.email)],
        )
        .await?;
    if !domains.is_some_and(|d| domain_allowed(&d, input.domain))
        || taken
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()?
    {
        tx.rollback().await?;
        return Ok(JitOutcome::Skipped);
    }
    if tx
        .operation()
        .require_new_instance_billable_user(None, license)
        .await?
        .is_err()
    {
        tx.rollback().await?;
        return Ok(JitOutcome::SeatLimit);
    }
    let user = Uuid::now_v7();
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family JIT user")
    };
    family
        .execute(
            "INSERT INTO users (id,email,password_hash,given_name) VALUES (?1,?2,NULL,?3)",
            &[
                Cell::uuid(user),
                Cell::text(input.email),
                Cell::text(input.given_name),
            ],
        )
        .await?;
    family
        .execute(
            "INSERT INTO memberships (workspace_id,user_id,role) VALUES (?1,?2,'member')",
            &[Cell::uuid(input.workspace_id), Cell::uuid(user)],
        )
        .await?;
    if !family_insert_link(
        &mut tx.operation(),
        &NewLink {
            user_id: user,
            provider: "generic",
            subject: input.subject,
            issuer: input.issuer,
            email: Some(input.email),
            workspace_id: Some(input.workspace_id),
        },
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(JitOutcome::Skipped);
    }
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
    Ok(JitOutcome::Joined(user))
}

fn family_link_row(r: &super::codec::FamilyRow) -> Result<LinkRow, sqlx::Error> {
    Ok(LinkRow {
        id: r.cell(0)?.id()?,
        user_id: r.cell(1)?.id()?,
        provider: r.cell(2)?.string()?,
        email: r.cell(3)?.optional(Cell::string)?,
        created_at: r.cell(4)?.datetime()?,
    })
}
async fn family_links(
    op: &mut OperationTx<'_, '_>,
    user: Uuid,
) -> Result<Vec<LinkRow>, sqlx::Error> {
    let OperationTx::SqliteFamily(family) = op else {
        unreachable!("family identity links")
    };
    family.query("SELECT id,user_id,provider,email,created_at FROM identity_links WHERE user_id=?1 ORDER BY created_at,id",&[Cell::uuid(user)]).await?.iter().map(family_link_row).collect()
}
async fn family_insert_link(
    op: &mut OperationTx<'_, '_>,
    link: &NewLink<'_>,
) -> Result<bool, sqlx::Error> {
    let previous = op.set_system().await?;
    if let Some(workspace) = link.workspace_id {
        op.set_tenant(workspace).await?;
    }
    let id = Uuid::now_v7();
    let OperationTx::SqliteFamily(family) = op else {
        unreachable!("family identity insert")
    };
    family.require_writer()?;
    family.require_system_context()?;
    let inserted=family.execute("INSERT INTO identity_links (id,user_id,provider,provider_user_id,email,issuer) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",&[Cell::uuid(id),Cell::uuid(link.user_id),Cell::text(link.provider),Cell::text(link.subject),Cell::optional_text(link.email),Cell::text(link.issuer)]).await?==1;
    if inserted {
        op.append_event(EventAppend{id:Uuid::now_v7(),workspace_id:link.workspace_id,actor_user_id:Some(link.user_id),verb:"identity.linked".into(),target_type:Some("identity_link".into()),target_id:Some(id),payload:json!({"userId":link.user_id.to_string(),"provider":link.provider,"email":link.email})}).await?;
    }
    op.restore_system(previous).await?;
    Ok(inserted)
}
fn family_workspace_oidc_row(r: &super::codec::FamilyRow) -> Result<WorkspaceOidcRow, sqlx::Error> {
    Ok(WorkspaceOidcRow {
        workspace_id: r.cell(0)?.id()?,
        issuer: r.cell(1)?.string()?,
        client_id: r.cell(2)?.string()?,
        client_secret: r.cell(3)?.string()?,
        label: r.cell(4)?.string()?,
    })
}
async fn family_workspace_oidc(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
) -> Result<Option<WorkspaceOidcRow>, sqlx::Error> {
    let OperationTx::SqliteFamily(family) = op else {
        unreachable!("family workspace OIDC row")
    };
    family.require_tenant(workspace)?;
    family.query("SELECT workspace_id,issuer,client_id,client_secret,label FROM workspace_oidc WHERE workspace_id=?1",&[Cell::uuid(workspace)]).await?.first().map(family_workspace_oidc_row).transpose()
}
async fn family_require_manage(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    credential: &ManageCredential,
) -> Result<Result<(), ManageError>, sqlx::Error> {
    op.set_tenant(workspace).await?;
    let live = match credential {
        ManageCredential::Session(id) => op.auth_session_live(actor, *id).await?,
        ManageCredential::ApiToken { id, hash } => {
            op.recheck_api_token_scope(
                workspace,
                actor,
                *id,
                hash,
                crate::auth::scopes::ApiTokenScope::WorkspaceManage,
            )
            .await?
        }
    };
    if !live {
        return Ok(Err(ManageError::SessionGone));
    }
    if !op.workspace_is_live(workspace).await? {
        return Ok(Err(ManageError::NotFound));
    }
    Ok(match op.membership_role(workspace, actor, false).await? {
        None => Err(ManageError::NotFound),
        Some(r) if r.at_least(WorkspaceRole::Admin) => Ok(()),
        Some(_) => Err(ManageError::Forbidden),
    })
}
