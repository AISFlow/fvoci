//! Share links: creation/listing/revocation by members and public read access
//! by token.
//!
//! Source `packages/core/src/share.ts`. Tokens are 256-bit random values; only
//! their SHA-256 hex is stored and the app role cannot read that column
//! (lookup goes through `fvoci.app_share_link_by_token_hash`). Every public
//! request re-resolves the token (expiry, revocation = row deleted), rechecks
//! workspace liveness and the shared root's current state, and recomputes the
//! visible subtree inside the same tenant transaction that reads content.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token::{hash_token, new_token, token_hashes_eq};
use crate::db::context::{
    lock_membership_users, recheck_session, restore_system, session_is_live, set_system, set_tenant,
};
use crate::db::documents::{document_permission, membership_role, workspace_is_live};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::WorkspaceRole;
use crate::projects::ProjectPermission;

/// Source `settings.share` default (`enabled`, 7 days default, 365 max). The
/// instance settings store is not ported, so the policy is fixed at the default.
pub const SHARE_DEFAULT_EXPIRES_DAYS: i64 = 7;
pub const SHARE_MAX_EXPIRES_DAYS: i64 = 365;
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

/// Source `expiresAtFromDays`: default 7, integer 1..=max.
pub fn share_expiry(days: Option<i64>, now: DateTime<Utc>) -> Result<DateTime<Utc>, ShareDbError> {
    let value = days.unwrap_or(SHARE_DEFAULT_EXPIRES_DAYS);
    if !(1..=SHARE_MAX_EXPIRES_DAYS).contains(&value) {
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

/// Source `createShareLink`: edit permission on the target document or project.
/// `affiliation` is set for the document-scoped routes.
pub async fn create_share_link(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    target: ShareTarget,
    expires_in_days: Option<i64>,
    affiliation: Option<DocumentAffiliation>,
) -> Result<Result<ShareLinkCreated, ShareDbError>, sqlx::Error> {
    let expires_at = match share_expiry(expires_in_days, Utc::now()) {
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
    .bind(share.workspace_id)
    .bind(root)
    .fetch_all(&mut **tx)
    .await
}

#[derive(Debug, Clone)]
pub struct SharePublicMeta {
    pub title: String,
    pub document_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
}

/// Source `getSharePublicMeta` (without the OG excerpt).
pub async fn share_public_meta(
    pool: &PgPool,
    raw_token: &str,
) -> Result<Option<SharePublicMeta>, sqlx::Error> {
    let Some(ShareTx { mut tx, share }) = open_share(pool, raw_token).await? else {
        return Ok(None);
    };
    let Some(root) = share_root(&mut tx, &share).await? else {
        tx.rollback().await?;
        return Ok(None);
    };
    let title = match share.project_id {
        Some(project_id) if share.document_id.is_none() => {
            sqlx::query_scalar::<_, String>(
                "SELECT name FROM fvoci.projects WHERE workspace_id = $1 AND id = $2",
            )
            .bind(share.workspace_id)
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?
        }
        _ => {
            sqlx::query_scalar::<_, String>(
                "SELECT title FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(share.workspace_id)
            .bind(root)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    Ok(Some(SharePublicMeta {
        title: fold_one_line(&title),
        document_id: Some(root),
        project_id: share.project_id,
        expires_at: share.expires_at,
    }))
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

#[derive(Debug, Clone)]
pub struct ShareAttachment {
    pub id: Uuid,
    pub name: String,
    pub mime: String,
    pub size_bytes: Option<i64>,
    pub image: bool,
    pub scan_status: String,
    pub storage_key: String,
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
        Uuid,
        String,
        String,
        Option<i64>,
        bool,
        String,
        String,
        String,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    );
    let row: Option<AttRow> = sqlx::query_as(
        r#"
        SELECT a.id, a.document_id, a.name, a.mime, a.size_bytes, a.image, a.scan_status,
               a.status, a.storage_key, a.created_at, a.completed_at
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
        created_at,
        completed_at,
    }))
}

// ---------------------------------------------------------------------------
// Public search hydration
// ---------------------------------------------------------------------------

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
        let tasks: Vec<(Uuid, String, Uuid, DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT t.id, t.title, t.project_id, date_trunc('milliseconds', t.updated_at)
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
        rows.extend(
            tasks
                .into_iter()
                .map(|(id, title, project_id, updated_at)| ShareSearchRow {
                    is_task: true,
                    id,
                    title: title.clone(),
                    body: title,
                    project_id: Some(project_id),
                    updated_at,
                }),
        );
    }
    tx.commit().await?;
    Ok(Some((share, rows)))
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
        assert_eq!(
            share_expiry(None, now).unwrap(),
            now + chrono::Duration::days(7)
        );
        assert!(share_expiry(Some(0), now).is_err());
        assert!(share_expiry(Some(366), now).is_err());
        assert!(share_expiry(Some(365), now).is_ok());
    }
}
