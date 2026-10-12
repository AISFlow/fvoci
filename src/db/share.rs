//! Share links: creation/listing/revocation by members and public read access
//! by token.
//!
//! Source `packages/core/src/share.ts`. Tokens are 256-bit random values; only
//! their SHA-256 hex is stored and the app role cannot read that column
//! (lookup goes through `fvoci.app_share_link_by_token_hash`). Every public
//! request re-resolves the token (expiry, revocation = row deleted), rechecks
//! workspace liveness and the shared root's current state, and recomputes the
//! visible subtree inside the same tenant transaction that reads content.

use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use unicode_normalization::UnicodeNormalization;

use crate::auth::token::{hash_token, new_token, token_hashes_eq};
use crate::collab::derived_body::extract_text;
use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, session_is_live, set_system, set_tenant,
};
use crate::db::documents::{document_permission, document_permissions};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::ProjectPermission;
use crate::settings::catalog::SharePolicy;

/// Tokens we issue are 43 chars; anything much longer is not ours and is not hashed.
const MAX_TOKEN_LEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareDbError {
    NotFound,
    Forbidden,
    InvalidInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareTarget {
    Document(Uuid),
    Project(Uuid),
}

/// Route affiliation of a document-scoped share route (source `affiliationFromParams`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentAffiliation {
    Workspace,
    Project(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLinkRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ShareLinkCreated {
    pub record: ShareLinkRecord,
    /// Raw token, returned once to the creator inside the share URL.
    pub token: String,
}

type RecordRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    DateTime<Utc>,
    DateTime<Utc>,
);

fn record_from_row(row: RecordRow) -> ShareLinkRecord {
    let (id, workspace_id, document_id, project_id, expires_at, created_at) = row;
    ShareLinkRecord {
        id,
        workspace_id,
        document_id,
        project_id,
        expires_at,
        created_at,
    }
}

/// Source `expiresAtFromDays`: refused while the instance policy is disabled;
/// otherwise the policy default, integer 1..=policy max.
pub fn share_expiry(
    days: Option<i64>,
    policy: &SharePolicy,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ShareDbError> {
    if !policy.enabled {
        return Err(ShareDbError::InvalidInput);
    }
    let value = days.unwrap_or(policy.default_expires_days);
    if !(1..=policy.max_expires_days).contains(&value) {
        return Err(ShareDbError::InvalidInput);
    }
    Ok(now + chrono::Duration::days(value))
}

/// Source `foldOneLine`: drop bidi controls and U+FEFF, collapse whitespace and
/// control runs to one space, trim.
pub fn fold_one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        ) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c);
    }
    out
}

async fn begin_member_tx<'a>(
    pool: &'a PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    lock_for_write: bool,
) -> Result<Result<(Transaction<'a, Postgres>, WorkspaceRole), ShareDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let live = if lock_for_write {
        lock_membership_users(&mut tx, &[actor_user_id]).await?;
        recheck_session(&mut tx, actor_user_id, credential_id).await?
    } else {
        session_is_live(&mut tx, actor_user_id, credential_id).await?
    };
    if !live {
        tx.rollback().await?;
        return Ok(Err(ShareDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ShareDbError::NotFound));
    }
    let Some(role) = membership_role(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ShareDbError::Forbidden));
    };
    Ok(Ok((tx, role)))
}

/// Current permission on a live document under the route's affiliation.
/// Affiliation mismatch and trashed/missing documents are `None`.
async fn document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    affiliation: Option<DocumentAffiliation>,
) -> Result<ProjectPermission, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents
         WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id,)) = row else {
        return Ok(ProjectPermission::None);
    };
    match (affiliation, project_id) {
        (Some(DocumentAffiliation::Workspace), Some(_)) => return Ok(ProjectPermission::None),
        (Some(DocumentAffiliation::Project(route)), actual) if actual != Some(route) => {
            return Ok(ProjectPermission::None)
        }
        _ => {}
    }
    match project_id {
        None => document_permission(tx, workspace_id, actor_user_id, document_id, true).await,
        Some(project_id) => {
            Ok(
                project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
                    .await?
                    .unwrap_or(ProjectPermission::None),
            )
        }
    }
}

/// A link serves its root's whole live subtree ([`visible_document_ids`]), but a
/// wiki grant is not inherited: edit on a page is not view on its subpages.
/// True when `actor` can view every document a link on the wiki `root` would
/// expose. Project documents take the project's permission, which the root
/// check already covers.
async fn sees_whole_wiki_subtree(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    root: Uuid,
) -> Result<bool, sqlx::Error> {
    let project: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents
         WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(root)
    .fetch_optional(&mut **tx)
    .await?;
    if !matches!(project, Some((None,))) {
        return Ok(true);
    }
    let exposed = live_subtree_ids(tx, workspace_id, root).await?;
    let levels = document_permissions(tx, workspace_id, actor_user_id, &exposed, true).await?;
    Ok(exposed.iter().all(|id| {
        levels
            .get(id)
            .is_some_and(|level| level.at_least(ProjectPermission::View))
    }))
}

/// Source `createShareLink`: edit permission on the target document or project,
/// and (FVOCI, not in the source) view on every wiki page the link would
/// expose. `affiliation` is set for the document-scoped routes.
#[allow(clippy::too_many_arguments)]
pub async fn create_share_link(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    target: ShareTarget,
    expires_in_days: Option<i64>,
    policy: &SharePolicy,
    affiliation: Option<DocumentAffiliation>,
) -> Result<Result<ShareLinkCreated, ShareDbError>, sqlx::Error> {
    let expires_at = match share_expiry(expires_in_days, policy, Utc::now()) {
        Ok(at) => at,
        Err(err) => return Ok(Err(err)),
    };
    let (mut tx, _role) =
        match begin_member_tx(pool, workspace_id, actor_user_id, credential_id, true).await? {
            Ok(v) => v,
            Err(err) => return Ok(Err(err)),
        };
    let permission = match target {
        ShareTarget::Document(document_id) => {
            let permission = document_access(
                &mut tx,
                workspace_id,
                actor_user_id,
                document_id,
                affiliation,
            )
            .await?;
            if !permission.at_least(ProjectPermission::View) {
                tx.rollback().await?;
                return Ok(Err(ShareDbError::NotFound));
            }
            permission
        }
        ShareTarget::Project(project_id) => {
            match project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id).await?
            {
                Some(permission) if permission.at_least(ProjectPermission::View) => permission,
                _ => {
                    tx.rollback().await?;
                    return Ok(Err(ShareDbError::NotFound));
                }
            }
        }
    };
    if !permission.at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(ShareDbError::Forbidden));
    }
    // Same answer as a missing edit right, so which pages are hidden is not
    // revealed. An actor who knows they hold edit on the root does learn that
    // some page below it is hidden; any refusal tells that much.
    if let ShareTarget::Document(root) = target {
        if !sees_whole_wiki_subtree(&mut tx, workspace_id, actor_user_id, root).await? {
            tx.rollback().await?;
            return Ok(Err(ShareDbError::Forbidden));
        }
    }
    let (document_id, project_id) = match target {
        ShareTarget::Document(id) => (Some(id), None),
        ShareTarget::Project(id) => (None, Some(id)),
    };
    let issued = new_token();
    let row: RecordRow = sqlx::query_as(
        r#"
        INSERT INTO fvoci.share_links (
            id, workspace_id, user_id, token_hash, document_id, project_id, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, workspace_id, document_id, project_id,
                  date_trunc('milliseconds', expires_at), date_trunc('milliseconds', created_at)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(actor_user_id)
    .bind(&issued.hash)
    .bind(document_id)
    .bind(project_id)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(ShareLinkCreated {
        record: record_from_row(row),
        token: issued.token,
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn create_share_link_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: ShareTarget,
    days: Option<i64>,
    policy: &SharePolicy,
    affiliation: Option<DocumentAffiliation>,
) -> Result<Result<ShareLinkCreated, ShareDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_share_link(
            pool,
            ws,
            actor,
            credential,
            target,
            days,
            policy,
            affiliation,
        )
        .await;
    }
    let expires = match share_expiry(days, policy, Utc::now()) {
        Ok(e) => e,
        Err(e) => return Ok(Err(e)),
    };
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_share_actor(&mut op, ws, actor, credential).await? {
            return Ok(Err(e));
        }
        let permission = match target {
            ShareTarget::Document(doc) => family_share_document_access(&mut op, ws, actor, doc, affiliation).await?,
            ShareTarget::Project(p) => op.project_permission_by_id(ws, actor, p).await?.unwrap_or(ProjectPermission::None),
        };
        if !permission.at_least(ProjectPermission::View) {
            return Ok(Err(ShareDbError::NotFound));
        }
        if !permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(ShareDbError::Forbidden));
        }
        if let ShareTarget::Document(root) = target {
            let wiki = {
                let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
                let rows = f.query("SELECT project_id IS NULL FROM documents WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(ws), Cell::uuid(root)]).await?;
                rows.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.boolean()?
            };
            if wiki {
                for doc in family_share_subtree(&mut op, ws, root).await? {
                    if !op.document_permission(ws, actor, doc, true).await?.at_least(ProjectPermission::View) {
                        return Ok(Err(ShareDbError::Forbidden));
                    }
                }
            }
        }
        let (doc, project) = match target {
            ShareTarget::Document(d) => (Some(d), None),
            ShareTarget::Project(p) => (None, Some(p)),
        };
        let token = new_token();
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("INSERT INTO share_links(id,workspace_id,user_id,token_hash,document_id,project_id,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7) RETURNING id,workspace_id,document_id,project_id,(expires_at/1000)*1000,(created_at/1000)*1000", &[Cell::uuid(Uuid::now_v7()), Cell::uuid(ws), Cell::uuid(actor), Cell::text(token.hash), Cell::optional_uuid(doc), Cell::optional_uuid(project), Cell::instant(expires)?]).await?;
        Ok(Ok(ShareLinkCreated { record: family_share_record(rows.first().ok_or(sqlx::Error::RowNotFound)?)?, token: token.token }))
    }.await;
    finish_share(tx, result, true).await
}

/// Source `listShareLinks`: workspace managers (owner/admin) see every link,
/// others only their own. `document` narrows to one document after checking
/// the actor can view it under the route affiliation.
pub async fn list_share_links(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    document: Option<(Uuid, DocumentAffiliation)>,
) -> Result<Result<Vec<ShareLinkRecord>, ShareDbError>, sqlx::Error> {
    let (mut tx, role) =
        match begin_member_tx(pool, workspace_id, actor_user_id, credential_id, false).await? {
            Ok(v) => v,
            Err(err) => return Ok(Err(err)),
        };
    if let Some((document_id, affiliation)) = document {
        let permission = document_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            document_id,
            Some(affiliation),
        )
        .await?;
        if !permission.at_least(ProjectPermission::View) {
            tx.rollback().await?;
            return Ok(Err(ShareDbError::NotFound));
        }
    }
    let manager = matches!(role, WorkspaceRole::Owner | WorkspaceRole::Admin);
    let rows: Vec<RecordRow> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, document_id, project_id,
               date_trunc('milliseconds', expires_at), date_trunc('milliseconds', created_at)
        FROM fvoci.share_links
        WHERE workspace_id = $1
          AND ($2 OR user_id = $3)
          AND ($4::uuid IS NULL OR document_id = $4)
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(manager)
    .bind(actor_user_id)
    .bind(document.map(|(id, _)| id))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows.into_iter().map(record_from_row).collect()))
}

pub async fn list_share_links_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Option<(Uuid, DocumentAffiliation)>,
) -> Result<Result<Vec<ShareLinkRecord>, ShareDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_share_links(pool, ws, actor, credential, document).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let role = match family_share_actor(&mut op, ws, actor, credential).await? {
            Ok(r) => r,
            Err(e) => return Ok(Err(e)),
        };
        if let Some((doc, affiliation)) = document {
            if !family_share_document_access(&mut op, ws, actor, doc, Some(affiliation)).await?.at_least(ProjectPermission::View) {
                return Ok(Err(ShareDbError::NotFound));
            }
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT id,workspace_id,document_id,project_id,(expires_at/1000)*1000,(created_at/1000)*1000 FROM share_links WHERE workspace_id=?1 AND (?2 OR user_id=?3) AND (?4 IS NULL OR document_id=?4) ORDER BY created_at DESC,id DESC", &[Cell::uuid(ws), Cell::Integer(i64::from(role.at_least(WorkspaceRole::Admin))), Cell::uuid(actor), Cell::optional_uuid(document.map(|(d, _)| d))]).await?;
        Ok(Ok(rows.iter().map(family_share_record).collect::<Result<Vec<_>, _>>()?))
    }.await;
    finish_share(tx, result, false).await
}

/// Source `revokeShareLink`: the creator, or a workspace manager. The row is
/// deleted, so the token stops resolving on the next request.
pub async fn revoke_share_link(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    share_id: Uuid,
) -> Result<Result<(), ShareDbError>, sqlx::Error> {
    let (mut tx, role) =
        match begin_member_tx(pool, workspace_id, actor_user_id, credential_id, true).await? {
            Ok(v) => v,
            Err(err) => return Ok(Err(err)),
        };
    // The app role has no UPDATE on share_links (so no row locks): one guarded
    // DELETE decides. Someone else's link reads as missing for non-managers.
    let manager = matches!(role, WorkspaceRole::Owner | WorkspaceRole::Admin);
    let removed: Option<(Uuid,)> = sqlx::query_as(
        r#"
        DELETE FROM fvoci.share_links
        WHERE workspace_id = $1 AND id = $2 AND ($3 OR user_id = $4)
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(share_id)
    .bind(manager)
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if removed.is_none() {
        tx.rollback().await?;
        return Ok(Err(ShareDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn revoke_share_link_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    share: Uuid,
) -> Result<Result<(), ShareDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return revoke_share_link(pool, ws, actor, credential, share).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        let role = match family_share_actor(&mut op, ws, actor, credential).await? {
            Ok(r) => r,
            Err(e) => return Ok(Err(e)),
        };
        let OperationTx::SqliteFamily(f) = &mut op else {
            unreachable!()
        };
        let n = f
            .execute(
                "DELETE FROM share_links WHERE workspace_id=?1 AND id=?2 AND (?3 OR user_id=?4)",
                &[
                    Cell::uuid(ws),
                    Cell::uuid(share),
                    Cell::Integer(i64::from(role.at_least(WorkspaceRole::Admin))),
                    Cell::uuid(actor),
                ],
            )
            .await?;
        Ok(if n == 0 {
            Err(ShareDbError::NotFound)
        } else {
            Ok(())
        })
    }
    .await;
    finish_share(tx, result, true).await
}

// ---------------------------------------------------------------------------
// Public access
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShare {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
}

type ShareLookupRow = (
    Uuid,
    Uuid,
    String,
    Option<Uuid>,
    Option<Uuid>,
    DateTime<Utc>,
);

/// Token → share row. Unknown, malformed, expired and revoked (deleted) tokens
/// are all `None`.
async fn lookup_token(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<ResolvedShare>, sqlx::Error> {
    if raw_token.is_empty() || raw_token.len() > MAX_TOKEN_LEN {
        return Ok(None);
    }
    let computed = hash_token(raw_token);
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let row: Option<ShareLookupRow> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, token_hash, document_id, project_id,
               date_trunc('milliseconds', expires_at)
        FROM fvoci.app_share_link_by_token_hash($1)
        "#,
    )
    .bind(&computed)
    .fetch_optional(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    let Some((id, workspace_id, stored, document_id, project_id, expires_at)) = row else {
        return Ok(None);
    };
    if !token_hashes_eq(&stored, &computed) || expires_at <= Utc::now() {
        return Ok(None);
    }
    Ok(Some(ResolvedShare {
        id,
        workspace_id,
        document_id,
        project_id,
        expires_at,
    }))
}

/// Tenant transaction for one public request, with the share re-resolved and
/// the workspace checked live. Callers read everything else inside it.
pub struct ShareTx {
    pub tx: Transaction<'static, Postgres>,
    pub share: ResolvedShare,
}

pub async fn open_share(pool: &PgPool, raw_token: &str) -> Result<Option<ShareTx>, sqlx::Error> {
    let Some(share) = lookup_token(pool, raw_token).await? else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, share.workspace_id).await?;
    if !workspace_is_live(&mut tx, share.workspace_id).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    // The row may have been revoked between the lookup and this snapshot.
    let still: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.share_links
         WHERE workspace_id = $1 AND id = $2 AND expires_at > now()",
    )
    .bind(share.workspace_id)
    .bind(share.id)
    .fetch_optional(&mut *tx)
    .await?;
    if still.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    Ok(Some(ShareTx { tx, share }))
}

/// Root document of the share: the shared document (wiki or project document),
/// or the project's root document for a live project share. `None` when the
/// root is gone or trashed. The subtree below follows the root's project.
async fn share_root(
    tx: &mut Transaction<'_, Postgres>,
    share: &ResolvedShare,
) -> Result<Option<Uuid>, sqlx::Error> {
    let root = match (share.document_id, share.project_id) {
        (Some(document_id), _) => Some(document_id),
        (None, Some(project_id)) => sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT root_document_id FROM fvoci.projects
             WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(share.workspace_id)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .flatten(),
        (None, None) => None,
    };
    let Some(root) = root else {
        return Ok(None);
    };
    let live: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.documents
         WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
           AND ($3::uuid IS NULL OR project_id = $3)",
    )
    .bind(share.workspace_id)
    .bind(root)
    .bind(share.project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(id,)| id))
}

/// Source `visibleDocIds`: the live subtree under the share root, in the root's
/// project scope. A document under a trashed ancestor inside the subtree is
/// excluded too.
pub async fn visible_document_ids(
    tx: &mut Transaction<'_, Postgres>,
    share: &ResolvedShare,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let Some(root) = share_root(tx, share).await? else {
        return Ok(Vec::new());
    };
    live_subtree_ids(tx, share.workspace_id, root).await
}

/// The live subtree under `root` (inclusive, when `root` is live) in the root's
/// project scope, minus documents under a trashed ancestor inside the subtree:
/// exactly what a share rooted at `root` serves.
async fn live_subtree_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    root: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        WITH root AS (
            SELECT path, project_id FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        )
        SELECT d.id
        FROM fvoci.documents d
        CROSS JOIN root
        WHERE d.workspace_id = $1
          AND d.deleted_at IS NULL
          AND d.project_id IS NOT DISTINCT FROM root.project_id
          AND (d.path = root.path OR substr(d.path, 1, length(root.path) + 1) = root.path || '.')
          AND NOT EXISTS (
              SELECT 1 FROM fvoci.documents x
              WHERE x.workspace_id = $1
                AND x.deleted_at IS NOT NULL
                AND substr(d.path, 1, length(x.path) + 1) = x.path || '.'
                AND (x.path = root.path
                     OR substr(x.path, 1, length(root.path) + 1) = root.path || '.')
          )
        ORDER BY d.sort_key COLLATE "C", d.id
        "#,
    )
    .bind(workspace_id)
    .bind(root)
    .fetch_all(&mut **tx)
    .await
}

#[derive(Debug, Clone)]
pub struct SharePublicMeta {
    pub title: String,
    /// Link-unfurl one-liner; empty without `with_excerpt` and for project shares.
    pub excerpt: String,
    pub document_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
}

/// Source `EXCERPT_MAX_CHARS` (UTF-16 code units, like `String.slice`).
pub const SHARE_EXCERPT_MAX_UTF16: usize = 200;

/// Source `shareExcerpt`: folded plain text of a Tiptap doc, cut to 200 UTF-16
/// units. The cut never splits a surrogate pair (JS `slice` could leave half
/// of one, which HTML output would turn into U+FFFD).
pub fn share_excerpt(content_json: &Value) -> String {
    if !crate::share_render::is_tiptap_doc(content_json) {
        return String::new();
    }
    let folded = fold_one_line(&extract_text(content_json));
    let mut units = 0;
    let mut end = folded.len();
    for (index, c) in folded.char_indices() {
        if units + c.len_utf16() > SHARE_EXCERPT_MAX_UTF16 {
            end = index;
            break;
        }
        units += c.len_utf16();
    }
    folded[..end].to_string()
}

/// Source `getSharePublicMeta`; `with_excerpt` is the OG head path only (it
/// walks the whole document body).
pub async fn share_public_meta(
    pool: &PgPool,
    raw_token: &str,
    with_excerpt: bool,
) -> Result<Option<SharePublicMeta>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    let Some(root) = share_root(&mut tx, &share).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    let (title, excerpt) = match share.project_id {
        Some(project_id) if share.document_id.is_none() => {
            let name = sqlx::query_scalar::<_, String>(
                "SELECT name FROM fvoci.projects WHERE workspace_id = $1 AND id = $2",
            )
            .bind(share.workspace_id)
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
            (name, String::new())
        }
        _ if with_excerpt => {
            let (title, content_json) = sqlx::query_as::<_, (String, Value)>(
                "SELECT title, content_json FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(share.workspace_id)
            .bind(root)
            .fetch_one(&mut *tx)
            .await?;
            (title, share_excerpt(&content_json))
        }
        _ => {
            let title = sqlx::query_scalar::<_, String>(
                "SELECT title FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(share.workspace_id)
            .bind(root)
            .fetch_one(&mut *tx)
            .await?;
            (title, String::new())
        }
    };
    tx.commit().await?;
    Ok(Some(SharePublicMeta {
        title: fold_one_line(&title),
        excerpt,
        document_id: Some(root),
        project_id: share.project_id,
        expires_at: share.expires_at,
    }))
}

pub async fn share_public_meta_backend(
    backend: &Backend,
    raw: &str,
    with_excerpt: bool,
) -> Result<Option<SharePublicMeta>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return share_public_meta(pool, raw, with_excerpt).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        let ws = scope.share.workspace_id;
        let OperationTx::SqliteFamily(f) = &mut op else {
            unreachable!()
        };
        let (title, excerpt) =
            if let (Some(project), None) = (scope.share.project_id, scope.share.document_id) {
                let rows = f
                    .query(
                        "SELECT name FROM projects WHERE workspace_id=?1 AND id=?2",
                        &[Cell::uuid(ws), Cell::uuid(project)],
                    )
                    .await?;
                (
                    rows.first()
                        .ok_or(sqlx::Error::RowNotFound)?
                        .cell(0)?
                        .string()?,
                    String::new(),
                )
            } else {
                let rows = f
                    .query(
                        "SELECT title,content_json FROM documents WHERE workspace_id=?1 AND id=?2",
                        &[Cell::uuid(ws), Cell::uuid(scope.root)],
                    )
                    .await?;
                let r = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                (
                    r.cell(0)?.string()?,
                    if with_excerpt {
                        share_excerpt(&r.cell(1)?.value()?)
                    } else {
                        String::new()
                    },
                )
            };
        Ok(Some(SharePublicMeta {
            title: fold_one_line(&title),
            excerpt,
            document_id: Some(scope.root),
            project_id: scope.share.project_id,
            expires_at: scope.share.expires_at,
        }))
    }
    .await;
    finish_public_share(tx, result).await
}

#[derive(Debug, Clone)]
pub struct ShareDocument {
    pub id: Uuid,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub content_json: Value,
}

/// Source `getShareDocument`: the root when `document_id` is `None`, otherwise
/// a document inside the visible subtree.
pub async fn share_document(
    pool: &PgPool,
    raw_token: &str,
    document_id: Option<Uuid>,
) -> Result<Option<ShareDocument>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    let target = match document_id {
        Some(requested) => {
            let visible = visible_document_ids(&mut tx, &share).await?;
            visible.contains(&requested).then_some(requested)
        }
        None => share_root(&mut tx, &share).await?,
    };
    let Some(target) = target else {
        tx.rollback().await?;
        return Ok(None);
    };
    let row: Option<(Uuid, String, DateTime<Utc>, Value)> = sqlx::query_as(
        r#"
        SELECT id, title, date_trunc('milliseconds', updated_at), content_json
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(share.workspace_id)
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(
        row.map(|(id, title, updated_at, content_json)| ShareDocument {
            id,
            title,
            updated_at,
            content_json,
        }),
    )
}

pub async fn share_document_backend(
    backend: &Backend,
    raw: &str,
    document: Option<Uuid>,
) -> Result<Option<ShareDocument>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return share_document(pool, raw, document).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        let ws = scope.share.workspace_id;
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let target = document.unwrap_or(scope.root);
        if !scope.visible.contains(&target) {
            return Ok(None);
        }
        let rows = f.query("SELECT id,title,(updated_at/1000)*1000,content_json FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL", &[Cell::uuid(ws), Cell::uuid(target)]).await?;
        rows.first().map(|r| Ok(ShareDocument { id: r.cell(0)?.id()?, title: r.cell(1)?.string()?, updated_at: r.cell(2)?.datetime()?, content_json: r.cell(3)?.value()? })).transpose()
    }.await;
    finish_public_share(tx, result).await
}

#[derive(Debug, Clone)]
pub struct ShareTreeNode {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub icon: Option<String>,
    pub path: String,
    pub sort_key: String,
    pub number: i32,
    pub status: String,
}

type TreeRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    String,
    i32,
    String,
);

/// Source `listShareTree`: tree nodes of the visible subtree only.
pub async fn share_tree(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<Vec<ShareTreeNode>>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    if share_root(&mut tx, &share).await?.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let visible = visible_document_ids(&mut tx, &share).await?;
    let rows: Vec<TreeRow> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, parent_id, project_id, title, icon, path, sort_key, number, status
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NULL AND id = ANY($2)
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(share.workspace_id)
    .bind(&visible)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(
        rows.into_iter()
            .map(
                |(
                    id,
                    workspace_id,
                    parent_id,
                    project_id,
                    title,
                    icon,
                    path,
                    sort_key,
                    number,
                    status,
                )| ShareTreeNode {
                    id,
                    workspace_id,
                    parent_id,
                    project_id,
                    title,
                    icon,
                    path,
                    sort_key,
                    number,
                    status,
                },
            )
            .collect(),
    ))
}

pub async fn share_tree_backend(
    backend: &Backend,
    raw: &str,
) -> Result<Option<Vec<ShareTreeNode>>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return share_tree(pool, raw).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        let ws = scope.share.workspace_id;
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT id,workspace_id,parent_id,project_id,title,icon,path,sort_key,number,status FROM documents WHERE workspace_id=?1 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY,id", &[Cell::uuid(ws)]).await?;
        let mut nodes = Vec::new();
        for r in rows {
            let id = r.cell(0)?.id()?;
            if scope.visible.contains(&id) {
                nodes.push(ShareTreeNode { id, workspace_id: r.cell(1)?.id()?, parent_id: r.cell(2)?.optional(Cell::id)?, project_id: r.cell(3)?.optional(Cell::id)?, title: r.cell(4)?.string()?, icon: r.cell(5)?.optional(Cell::string)?, path: r.cell(6)?.string()?, sort_key: r.cell(7)?.string()?, number: r.cell(8)?.int32()?, status: r.cell(9)?.string()? });
            }
        }
        Ok(Some(nodes))
    }.await;
    finish_public_share(tx, result).await
}

#[derive(Debug, Clone)]
pub struct ShareAttachment {
    pub id: Uuid,
    pub name: String,
    pub mime: String,
    pub size_bytes: Option<i64>,
    pub image: bool,
    pub scan_status: String,
    pub storage_key: String,
    pub variants: Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Source `openShareAttachment`: a stored, not-infected attachment whose parent
/// document is live and inside the visible subtree.
pub async fn share_attachment(
    pool: &PgPool,
    raw_token: &str,
    attachment_id: Uuid,
) -> Result<Option<ShareAttachment>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    type AttRow = (
        Uuid,
        Option<Uuid>,
        String,
        String,
        Option<i64>,
        bool,
        String,
        String,
        String,
        Value,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    );
    let row: Option<AttRow> = sqlx::query_as(
        r#"
        SELECT a.id, a.document_id, a.name, a.mime, a.size_bytes, a.image, a.scan_status,
               a.status, a.storage_key, a.variants, a.created_at, a.completed_at
        FROM fvoci.attachments a
        WHERE a.workspace_id = $1 AND a.id = $2
        "#,
    )
    .bind(share.workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((
        id,
        document_id,
        name,
        mime,
        size_bytes,
        image,
        scan_status,
        status,
        storage_key,
        variants,
        created_at,
        completed_at,
    )) = row
    else {
        tx.rollback().await?;
        return Ok(None);
    };
    if status != "stored" || scan_status == "infected" {
        tx.rollback().await?;
        return Ok(None);
    }
    // A task attachment (no document parent) is never reachable by a share.
    let Some(document_id) = document_id else {
        tx.rollback().await?;
        return Ok(None);
    };
    let visible = visible_document_ids(&mut tx, &share).await?;
    if !visible.contains(&document_id) {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.commit().await?;
    Ok(Some(ShareAttachment {
        id,
        name,
        mime,
        size_bytes,
        image,
        scan_status,
        storage_key,
        variants,
        created_at,
        completed_at,
    }))
}

pub async fn share_attachment_backend(
    backend: &Backend,
    raw: &str,
    attachment: Uuid,
) -> Result<Option<ShareAttachment>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return share_attachment(pool, raw, attachment).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        let ws = scope.share.workspace_id;
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT id,document_id,name,mime,size_bytes,image,scan_status,status,storage_key,variants,created_at,completed_at FROM attachments WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(ws), Cell::uuid(attachment)]).await?;
        let Some(r) = rows.first() else {
            return Ok(None);
        };
        if r.cell(7)?.string()? != "stored" || r.cell(6)?.string()? == "infected" || !r.cell(1)?.optional(Cell::id)?.is_some_and(|d| scope.visible.contains(&d)) {
            return Ok(None);
        }
        Ok(Some(ShareAttachment { id: r.cell(0)?.id()?, name: r.cell(2)?.string()?, mime: r.cell(3)?.string()?, size_bytes: r.cell(4)?.optional(Cell::integer)?, image: r.cell(5)?.boolean()?, scan_status: r.cell(6)?.string()?, storage_key: r.cell(8)?.string()?, variants: r.cell(9)?.value()?, created_at: r.cell(10)?.datetime()?, completed_at: r.cell(11)?.optional(Cell::datetime)? }))
    }.await;
    finish_public_share(tx, result).await
}

// ---------------------------------------------------------------------------
// Public search hydration
// ---------------------------------------------------------------------------

type TaskHitRow = (Uuid, String, Uuid, DateTime<Utc>, Value);

type DocHitRow = (Uuid, String, String, Option<Uuid>, DateTime<Utc>);

#[derive(Debug, Clone)]
pub struct ShareSearchRow {
    pub is_task: bool,
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub project_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
}

/// Search scope computed before the Meili call.
pub struct ShareSearchScope {
    pub share: ResolvedShare,
    pub visible: Vec<Uuid>,
}

pub async fn share_search_scope(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<ShareSearchScope>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    if share_root(&mut tx, &share).await?.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let visible = visible_document_ids(&mut tx, &share).await?;
    tx.commit().await?;
    Ok(Some(ShareSearchScope { share, visible }))
}

pub async fn share_search_scope_backend(
    backend: &Backend,
    raw: &str,
) -> Result<Option<ShareSearchScope>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return share_search_scope(pool, raw).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        Ok(Some(ShareSearchScope {
            share: scope.share,
            visible: scope.visible,
        }))
    }
    .await;
    finish_public_share(tx, result).await
}

/// Source `hydrateShareHits` + the post-hydrate recheck: the token is resolved
/// again and hits are kept only if they are in the *current* scope. Documents
/// must be in the visible subtree; tasks only for a project share and only in
/// that project (source `itemVisibleInShare`).
pub async fn hydrate_share_hits(
    pool: &PgPool,
    raw_token: &str,
    document_ids: &[Uuid],
    task_ids: &[Uuid],
) -> Result<Option<(ResolvedShare, Vec<ShareSearchRow>)>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    if share_root(&mut tx, &share).await?.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let visible = visible_document_ids(&mut tx, &share).await?;
    let wanted_docs: Vec<Uuid> = document_ids
        .iter()
        .copied()
        .filter(|id| visible.contains(id))
        .collect();
    let mut rows = Vec::new();
    if !wanted_docs.is_empty() {
        let docs: Vec<DocHitRow> = sqlx::query_as(
            r#"
            SELECT id, title, text, project_id, date_trunc('milliseconds', updated_at)
            FROM fvoci.documents
            WHERE workspace_id = $1 AND deleted_at IS NULL AND id = ANY($2)
            "#,
        )
        .bind(share.workspace_id)
        .bind(&wanted_docs)
        .fetch_all(&mut *tx)
        .await?;
        rows.extend(
            docs.into_iter()
                .map(|(id, title, body, project_id, updated_at)| ShareSearchRow {
                    is_task: false,
                    id,
                    title,
                    body,
                    project_id,
                    updated_at,
                }),
        );
    }
    if let (Some(project_id), None, false) =
        (share.project_id, share.document_id, task_ids.is_empty())
    {
        let tasks: Vec<TaskHitRow> = sqlx::query_as(
            r#"
            SELECT t.id, t.title, t.project_id, date_trunc('milliseconds', t.updated_at),
                   t.content_json
            FROM fvoci.tasks t
            INNER JOIN fvoci.projects p
                ON p.workspace_id = t.workspace_id AND p.id = t.project_id AND p.deleted_at IS NULL
            WHERE t.workspace_id = $1 AND t.project_id = $2
              AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND t.id = ANY($3)
            "#,
        )
        .bind(share.workspace_id)
        .bind(project_id)
        .bind(task_ids)
        .fetch_all(&mut *tx)
        .await?;
        // Source `task.text` = `extractText(contentJson).normalize("NFC")`.
        rows.extend(
            tasks
                .into_iter()
                .map(
                    |(id, title, project_id, updated_at, content_json)| ShareSearchRow {
                        is_task: true,
                        id,
                        title,
                        body: extract_text(&content_json).nfc().collect(),
                        project_id: Some(project_id),
                        updated_at,
                    },
                ),
        );
    }
    tx.commit().await?;
    Ok(Some((share, rows)))
}

pub async fn hydrate_share_hits_backend(
    backend: &Backend,
    raw: &str,
    documents: &[Uuid],
    tasks: &[Uuid],
) -> Result<Option<(ResolvedShare, Vec<ShareSearchRow>)>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return hydrate_share_hits(pool, raw, documents, tasks).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        let Some(scope) = family_share_scope(&mut op, raw).await? else {
            return Ok(None);
        };
        let ws = scope.share.workspace_id;
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let mut hits = Vec::new();
        for id in documents.iter().filter(|d| scope.visible.contains(d)) {
            let rows = f.query("SELECT id,title,text,project_id,(updated_at/1000)*1000 FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL", &[Cell::uuid(ws), Cell::uuid(*id)]).await?;
            if let Some(r) = rows.first() {
                hits.push(ShareSearchRow { is_task: false, id: r.cell(0)?.id()?, title: r.cell(1)?.string()?, body: r.cell(2)?.string()?, project_id: r.cell(3)?.optional(Cell::id)?, updated_at: r.cell(4)?.datetime()? });
            }
        }
        if let (Some(project), None) = (scope.share.project_id, scope.share.document_id) {
            for id in tasks {
                let rows = f.query("SELECT t.id,t.title,t.project_id,(t.updated_at/1000)*1000,t.content_json FROM tasks t JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id AND p.deleted_at IS NULL WHERE t.workspace_id=?1 AND t.project_id=?2 AND t.id=?3 AND t.deleted_at IS NULL AND t.archived_at IS NULL", &[Cell::uuid(ws), Cell::uuid(project), Cell::uuid(*id)]).await?;
                if let Some(r) = rows.first() {
                    hits.push(ShareSearchRow { is_task: true, id: r.cell(0)?.id()?, title: r.cell(1)?.string()?, body: extract_text(&r.cell(4)?.value()?).nfc().collect(), project_id: Some(r.cell(2)?.id()?), updated_at: r.cell(3)?.datetime()? });
                }
            }
        }
        Ok(Some((scope.share, hits)))
    }.await;
    finish_public_share(tx, result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_one_line_matches_source_rules() {
        assert_eq!(fold_one_line("  a\n\tb  "), "a b");
        assert_eq!(fold_one_line("ab\u{202E}cd"), "abcd");
        assert_eq!(fold_one_line("x\u{0007}y"), "x y");
        assert_eq!(fold_one_line("👨\u{200D}💻"), "👨\u{200D}💻");
        assert_eq!(fold_one_line("\u{FEFF}제목"), "제목");
    }

    #[test]
    fn share_expiry_bounds() {
        let now = Utc::now();
        let policy = SharePolicy::default();
        assert_eq!(
            share_expiry(None, &policy, now).unwrap(),
            now + chrono::Duration::days(7)
        );
        assert!(share_expiry(Some(0), &policy, now).is_err());
        assert!(share_expiry(Some(366), &policy, now).is_err());
        assert!(share_expiry(Some(365), &policy, now).is_ok());
        let narrow = SharePolicy {
            enabled: true,
            default_expires_days: 3,
            max_expires_days: 30,
        };
        assert_eq!(
            share_expiry(None, &narrow, now).unwrap(),
            now + chrono::Duration::days(3)
        );
        assert!(share_expiry(Some(31), &narrow, now).is_err());
        let disabled = SharePolicy {
            enabled: false,
            ..SharePolicy::default()
        };
        assert!(share_expiry(Some(1), &disabled, now).is_err());
    }
}

#[derive(Debug, thiserror::Error)]
#[error("share operation refused: {0:?}")]
struct ShareRefusal(ShareDbError);
async fn finish_share<T>(
    tx: DbTx,
    result: Result<Result<T, ShareDbError>, sqlx::Error>,
    write: bool,
) -> Result<Result<T, ShareDbError>, sqlx::Error> {
    if write && matches!(&result, Ok(Ok(_))) {
        tx.commit_with_cleanup()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    } else if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Err(e) => Some(Box::new(e)),
            Ok(Err(e)) => Some(Box::new(ShareRefusal(e))),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}
async fn finish_public_share<T>(
    tx: DbTx,
    result: Result<Option<T>, sqlx::Error>,
) -> Result<Option<T>, sqlx::Error> {
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        ));
    }
    result
}
async fn family_share_actor(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<WorkspaceRole, ShareDbError>, sqlx::Error> {
    op.set_tenant(ws).await?;
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(ShareDbError::Forbidden));
    }
    if !op.workspace_is_live(ws).await? {
        return Ok(Err(ShareDbError::NotFound));
    }
    Ok(op
        .membership_role(ws, actor, false)
        .await?
        .ok_or(ShareDbError::Forbidden))
}
fn family_share_record(r: &FamilyRow) -> Result<ShareLinkRecord, sqlx::Error> {
    Ok(ShareLinkRecord {
        id: r.cell(0)?.id()?,
        workspace_id: r.cell(1)?.id()?,
        document_id: r.cell(2)?.optional(Cell::id)?,
        project_id: r.cell(3)?.optional(Cell::id)?,
        expires_at: r.cell(4)?.datetime()?,
        created_at: r.cell(5)?.datetime()?,
    })
}
async fn family_share_document_access(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    actor: Uuid,
    doc: Uuid,
    affiliation: Option<DocumentAffiliation>,
) -> Result<ProjectPermission, sqlx::Error> {
    let rows = {
        let OperationTx::SqliteFamily(f) = op else {
            unreachable!()
        };
        f.query("SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(ws),Cell::uuid(doc)]).await?
    };
    let Some(r) = rows.first() else {
        return Ok(ProjectPermission::None);
    };
    let project = r.cell(0)?.optional(Cell::id)?;
    match (affiliation, project) {
        (Some(DocumentAffiliation::Workspace), Some(_)) => return Ok(ProjectPermission::None),
        (Some(DocumentAffiliation::Project(route)), actual) if actual != Some(route) => {
            return Ok(ProjectPermission::None)
        }
        _ => {}
    }
    if let Some(project) = project {
        return Ok(op
            .project_permission_by_id(ws, actor, project)
            .await?
            .unwrap_or(ProjectPermission::None));
    }
    op.document_permission(ws, actor, doc, true).await
}
async fn family_share_subtree(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    root: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let OperationTx::SqliteFamily(f) = op else {
        unreachable!()
    };
    f.require_tenant(ws)?;
    f.query("WITH root AS (SELECT path,project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL) SELECT d.id FROM documents d CROSS JOIN root WHERE d.workspace_id=?1 AND d.deleted_at IS NULL AND d.project_id IS root.project_id AND (d.path=root.path OR substr(d.path,1,length(root.path)+1)=root.path||'.') AND NOT EXISTS(SELECT 1 FROM documents x WHERE x.workspace_id=?1 AND x.deleted_at IS NOT NULL AND substr(d.path,1,length(x.path)+1)=x.path||'.' AND (x.path=root.path OR substr(x.path,1,length(root.path)+1)=root.path||'.')) ORDER BY d.sort_key COLLATE BINARY,d.id",&[Cell::uuid(ws),Cell::uuid(root)]).await?.iter().map(|r|r.cell(0)?.id()).collect()
}
struct FamilyShareScope {
    share: ResolvedShare,
    root: Uuid,
    visible: Vec<Uuid>,
}
async fn family_share_scope(
    op: &mut OperationTx<'_, '_>,
    raw: &str,
) -> Result<Option<FamilyShareScope>, sqlx::Error> {
    if raw.is_empty() || raw.len() > MAX_TOKEN_LEN {
        return Ok(None);
    }
    let hash = hash_token(raw);
    let previous = op.set_system().await?;
    let lookup= async {
        let OperationTx::SqliteFamily(f) = &mut *op else { unreachable!() };
        f.require_system_context()?;
        f.query("SELECT id,workspace_id,token_hash,document_id,project_id,(expires_at/1000)*1000 FROM share_links WHERE token_hash=?1", &[Cell::text(&hash)]).await
    }.await;
    op.restore_system(previous).await?;
    let rows = lookup?;
    let Some(r) = rows.first() else {
        return Ok(None);
    };
    let expires_at = r.cell(5)?.datetime()?;
    if !token_hashes_eq(&r.cell(2)?.string()?, &hash) || expires_at <= Utc::now() {
        return Ok(None);
    }
    let share = ResolvedShare {
        id: r.cell(0)?.id()?,
        workspace_id: r.cell(1)?.id()?,
        document_id: r.cell(3)?.optional(Cell::id)?,
        project_id: r.cell(4)?.optional(Cell::id)?,
        expires_at,
    };
    op.set_tenant(share.workspace_id).await?;
    if !op.workspace_is_live(share.workspace_id).await? {
        return Ok(None);
    }
    let root = {
        let OperationTx::SqliteFamily(f) = &mut *op else {
            unreachable!()
        };
        let root = if let Some(doc) = share.document_id {
            Some(doc)
        } else if let Some(project) = share.project_id {
            f.query("SELECT root_document_id FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(share.workspace_id),Cell::uuid(project)]).await?.first().map(|r|r.cell(0)?.optional(Cell::id)).transpose()?.flatten()
        } else {
            None
        };
        let Some(root) = root else {
            return Ok(None);
        };
        if f.query("SELECT 1 FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL AND (?3 IS NULL OR project_id=?3)",&[Cell::uuid(share.workspace_id),Cell::uuid(root),Cell::optional_uuid(share.project_id)]).await?.is_empty(){return Ok(None);}
        root
    };
    let visible = family_share_subtree(op, share.workspace_id, root).await?;
    Ok(Some(FamilyShareScope {
        share,
        root,
        visible,
    }))
}
