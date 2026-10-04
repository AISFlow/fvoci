use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, OperationTx};
use super::codec::{Cell, FamilyRow};

use crate::db::context::{
    begin_read, lock_membership_users, lock_tree, recheck_session, session_is_live, set_tenant,
};
use crate::db::group_grants::{
    group_document_grant_roles_select_sql, group_members_join_sql,
    guest_wiki_document_ids_select_sql,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{lock_project, project_permission, visible_project_sql};
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::{workspace_base_permission, ProjectMemberRole, ProjectPermission};

pub const MAX_TREE_DEPTH: i32 = 20;
pub const DOCUMENT_SCHEMA_VERSION: i32 = 2;
const DOCUMENT_TITLE_MAX: usize = 300;
const DOCUMENT_ICON_MAX: usize = 50;

const FRACTIONAL_ALPHABET: &[u8] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const FRACTIONAL_BASE: usize = 62;
const FRACTIONAL_MID: usize = 31;

#[derive(Debug)]
pub enum DocumentDbError {
    NotFound,
    Forbidden,
    AffiliationMismatch,
    DepthLimit,
    InvalidSortKey,
    Cycle,
    TrashedParent,
    RootDocumentTrash,
    RootDocumentMove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashChildrenMode {
    Trash,
    Reparent,
}

#[derive(Debug, Clone)]
pub struct TrashNode {
    pub id: Uuid,
    pub title: String,
    pub deleted_at: DateTime<Utc>,
    pub project_id: Option<Uuid>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FractionalError {
    InvalidKey { side: &'static str, key: String },
    OutOfOrder { a: String, b: String },
}

impl std::fmt::Display for FractionalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKey { side, key } => {
                write!(f, "fractional.between: invalid key {side}={key:?}")
            }
            Self::OutOfOrder { a, b } => {
                write!(f, "fractional.between: a must be < b (a={a:?}, b={b:?})")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMeta {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub number: i32,
    pub icon: Option<String>,
    pub path: String,
    pub parent_id: Option<Uuid>,
    pub sort_key: String,
    pub project_id: Option<Uuid>,
    pub status: String,
    pub schema_version: i32,
    pub version: i32,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub display_id: Option<String>,
    pub content_json: Value,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
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

#[derive(Debug, Clone)]
pub struct AncestorCrumb {
    pub id: Uuid,
    pub title: String,
    pub icon: Option<String>,
    pub path: String,
    pub project_id: Option<Uuid>,
    pub number: i32,
}

pub struct CreateDocumentInput<'a> {
    pub parent_id: Option<Uuid>,
    pub title: &'a str,
    pub icon: Option<Option<&'a str>>,
}

pub struct UpdateDocumentMetaInput<'a> {
    pub title: Option<&'a str>,
    pub icon: Option<Option<&'a str>>,
    pub status: Option<&'a str>,
}

type DocProjectDeletedParent = (Option<Uuid>, Option<DateTime<Utc>>, Option<Uuid>);
type DocProjectDeletedPathParent = (Option<Uuid>, Option<DateTime<Utc>>, String, Option<Uuid>);
type DocParentProjectDeleted = (Option<Uuid>, Option<Uuid>, Option<DateTime<Utc>>);

type DocumentRow = (
    Uuid,
    Uuid,
    String,
    i32,
    Option<String>,
    String,
    Option<Uuid>,
    String,
    Option<Uuid>,
    String,
    i32,
    i32,
    Uuid,
    DateTime<Utc>,
    DateTime<Utc>,
    Value,
);

/// Days a trashed document (or project) stays restorable before the maintenance
/// sweep purges it (source `TRASH_GC_AFTER_MS`, 30 days). Every retention check
/// runs in SQL against the database clock (`now()`), so restore, the trash
/// lists and the purge never disagree because of app/job host clock skew.
pub const TRASH_RETENTION_DAYS: i32 = 30;

/// Extra days the purge waits past the retention, so it never touches a row
/// that restore could still accept.
pub const TRASH_PURGE_MARGIN_DAYS: i32 = 1;

/// Trash rows stamped at or before `now() - TRASH_RETENTION_DAYS` are expired:
/// the purge may already have deleted their attachment objects, so restore
/// treats them as gone.
pub(crate) async fn trash_expired(
    tx: &mut Transaction<'_, Postgres>,
    deleted_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT $1::timestamptz <= now() - make_interval(days => $2)")
        .bind(deleted_at)
        .bind(TRASH_RETENTION_DAYS)
        .fetch_one(&mut **tx)
        .await
}

pub fn empty_document_json() -> Value {
    json!({"type":"doc","content":[{"type":"paragraph"}]})
}

pub fn to_path_label(id: Uuid) -> String {
    id.simple().to_string()
}

pub fn format_display_id(prefix: &str, number: i32) -> String {
    format!("{prefix}-{number}")
}

pub fn title_is_valid(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && utf16_len(trimmed) <= DOCUMENT_TITLE_MAX
}

pub fn icon_is_valid(icon: &str) -> bool {
    utf16_len(icon) <= DOCUMENT_ICON_MAX
}

pub fn status_is_valid(status: &str) -> bool {
    matches!(status, "draft" | "published" | "archived")
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub(crate) fn depth_of(path: &str) -> i32 {
    path.split('.').count() as i32
}

pub(crate) fn wiki_can_edit(role: Option<WorkspaceRole>) -> bool {
    matches!(
        role,
        Some(WorkspaceRole::Owner | WorkspaceRole::Admin | WorkspaceRole::Member)
    )
}

fn permission_can_view(permission: ProjectPermission) -> bool {
    permission >= ProjectPermission::View
}

fn permission_can_edit(permission: ProjectPermission) -> bool {
    permission >= ProjectPermission::Edit
}

/// Wiki document permission for HTTP and tree listing. Project documents return `None`.
/// Effective level is max(workspace base, group grants on `document_members`).
pub async fn document_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
    require_live: bool,
) -> Result<ProjectPermission, sqlx::Error> {
    OperationTx::Postgres(tx)
        .document_permission(workspace_id, user_id, document_id, require_live)
        .await
}

impl OperationTx<'_, '_> {
    pub(crate) async fn document_permission(
        &mut self,
        workspace_id: Uuid,
        user_id: Uuid,
        document_id: Uuid,
        require_live: bool,
    ) -> Result<ProjectPermission, sqlx::Error> {
        let role = self.membership_role(workspace_id, user_id, false).await?;
        let Some(role) = role else {
            return Ok(ProjectPermission::None);
        };
        let row = self.document_affiliation(workspace_id, document_id).await?;
        let Some((project_id, deleted_at)) = row else {
            return Ok(ProjectPermission::None);
        };
        if !wiki_document_eligible(project_id, deleted_at, require_live) {
            return Ok(ProjectPermission::None);
        }
        let base = workspace_base_permission(role);
        let roles = self
            .wiki_group_roles(workspace_id, document_id, user_id)
            .await?;
        let granted = roles
            .iter()
            .map(|r| grant_role_permission(r))
            .max()
            .unwrap_or(ProjectPermission::None);
        Ok(base.max(granted))
    }
}

/// (document id, project id, deleted_at, one group-grant role or none).
type DocumentGrantRow = (Uuid, Option<Uuid>, Option<DateTime<Utc>>, Option<String>);

/// Set-based [`document_permission`] for many documents in one statement (plus
/// the same single membership read). Every requested id is in the result;
/// missing, project and (with `require_live`) trashed documents are `None`.
pub async fn document_permissions(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
    require_live: bool,
) -> Result<HashMap<Uuid, ProjectPermission>, sqlx::Error> {
    let mut levels: HashMap<Uuid, ProjectPermission> = document_ids
        .iter()
        .map(|id| (*id, ProjectPermission::None))
        .collect();
    if levels.is_empty() {
        return Ok(levels);
    }
    let Some(role) = membership_role(tx, workspace_id, user_id).await? else {
        return Ok(levels);
    };
    // The grant arm is `group_document_grant_roles_select_sql` with the
    // document parameter widened to `= ANY($2)`; one row per (document, grant).
    let join = group_members_join_sql("dm", "gm");
    let rows: Vec<DocumentGrantRow> = sqlx::query_as(&format!(
        r#"
            SELECT d.id, d.project_id, d.deleted_at, g.role
            FROM fvoci.documents d
            LEFT JOIN (
                SELECT dm.document_id, dm.role
                FROM fvoci.document_members dm
                {join}
                WHERE dm.workspace_id = $1
                  AND dm.document_id = ANY($2)
                  AND gm.user_id = $3
                  AND dm.group_id IS NOT NULL
            ) g ON g.document_id = d.id
            WHERE d.workspace_id = $1 AND d.id = ANY($2)
            "#
    ))
    .bind(workspace_id)
    .bind(document_ids)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    let base = workspace_base_permission(role);
    for (document_id, project_id, deleted_at, grant) in rows {
        if !wiki_document_eligible(project_id, deleted_at, require_live) {
            continue;
        }
        let granted = grant
            .as_deref()
            .map(grant_role_permission)
            .unwrap_or(ProjectPermission::None);
        let level = levels.entry(document_id).or_insert(ProjectPermission::None);
        *level = (*level).max(base.max(granted));
    }
    Ok(levels)
}

/// Wiki affiliation rule shared by the single and bulk permission lookups.
fn wiki_document_eligible(
    project_id: Option<Uuid>,
    deleted_at: Option<DateTime<Utc>>,
    require_live: bool,
) -> bool {
    project_id.is_none() && !(require_live && deleted_at.is_some())
}

fn grant_role_permission(role: &str) -> ProjectPermission {
    ProjectMemberRole::parse(role)
        .map(ProjectMemberRole::permission)
        .unwrap_or(ProjectPermission::None)
}

async fn wiki_group_roles(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String,)>(&format!(
        r#"
        {}
        "#,
        group_document_grant_roles_select_sql(1, 2, 3)
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(role,)| role).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DocumentPermission {
    None,
    View,
    Edit,
}

impl DocumentPermission {
    pub fn at_least(self, min: Self) -> bool {
        self >= min
    }
}

/// Locks the document row and checks it is live (and its project not archived).
/// `expected_project_id` is the affiliation the caller authorized against, read
/// before this lock: a project caller has already locked that project, and a
/// wiki caller (`None`) never locks a project after the document row, so a
/// concurrent wiki -> project move is refused instead of inverting the
/// project -> document lock order used by collab and project writes.
pub(crate) async fn assert_document_writable(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    expected_project_id: Option<Uuid>,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id, deleted_at)) = row else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || project_id != expected_project_id {
        return Ok(Err(DocumentDbError::NotFound));
    }
    if let Some(project_id) = project_id {
        let locked = lock_project(tx, workspace_id, project_id).await?;
        let Some(locked) = locked else {
            return Ok(Err(DocumentDbError::NotFound));
        };
        if locked.status == "archived" {
            return Ok(Err(DocumentDbError::NotFound));
        }
    }
    Ok(Ok(()))
}

pub(crate) async fn record_document_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &str,
    target_id: Uuid,
    payload: Value,
    client_ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(target_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(target_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

pub(crate) fn row_to_meta(row: DocumentRow, with_display_id: bool) -> DocumentMeta {
    let (
        id,
        workspace_id,
        title,
        number,
        icon,
        path,
        parent_id,
        sort_key,
        project_id,
        status,
        schema_version,
        version,
        created_by,
        created_at,
        updated_at,
        content_json,
    ) = row;
    DocumentMeta {
        id,
        workspace_id,
        title,
        number,
        icon,
        path,
        parent_id,
        sort_key,
        project_id,
        status,
        schema_version,
        version,
        created_by,
        created_at,
        updated_at,
        display_id: if with_display_id {
            Some(format_display_id("WIKI", number))
        } else {
            None
        },
        content_json,
    }
}

pub async fn create_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let created = create_wiki_document_inner(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
        None,
    )
    .await?;
    Ok(created.map(|meta| meta.expect("unfenced create always returns the document")))
}

#[derive(Debug)]
pub enum CreateCommandError {
    Document(DocumentDbError),
    RequestMismatch,
}

/// Ordinary wiki creation has a stable command identity. Import/template and
/// personal-input callers keep their own command/fence and commit owner.
pub async fn create_wiki_document_command(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    command_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, CreateCommandError>, sqlx::Error> {
    create_wiki_document_command_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        actor_user_id,
        session_id,
        command_id,
        input,
        client_ip,
    )
    .await
}

pub async fn create_wiki_document_command_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    command_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, CreateCommandError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut operation = tx.operation();
    operation.set_tenant(workspace_id).await?;
    let result = create_wiki_command_in_tx(
        &mut operation,
        workspace_id,
        actor_user_id,
        session_id,
        command_id,
        input,
        client_ip,
    )
    .await?;
    if result.is_ok() {
        tx.commit()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

async fn create_wiki_command_in_tx(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    command_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, CreateCommandError>, sqlx::Error> {
    // The current credential and membership precede even a successful receipt
    // replay. The tenant tree lock serializes absence with all wiki creators.
    tx.lock_membership_users(&[actor_user_id]).await?;
    if !tx.recheck_session(actor_user_id, session_id).await? {
        return Ok(Err(CreateCommandError::Document(
            DocumentDbError::Forbidden,
        )));
    }
    tx.lock_tree(workspace_id).await?;
    if !tx.workspace_is_live(workspace_id).await? {
        return Ok(Err(CreateCommandError::Document(DocumentDbError::NotFound)));
    }
    if !wiki_can_edit(
        tx.membership_role(workspace_id, actor_user_id, true)
            .await?,
    ) {
        return Ok(Err(CreateCommandError::Document(
            DocumentDbError::Forbidden,
        )));
    }
    let hash = wiki_create_command_hash(workspace_id, actor_user_id, &input)?;
    let receipt = tx.wiki_create_receipt(workspace_id, command_id).await?;
    if let Some((actor, original_hash, target_id, original_result)) = receipt {
        if actor != actor_user_id || original_hash != hash {
            return Ok(Err(CreateCommandError::RequestMismatch));
        }
        let Some(target_id) = target_id else {
            return Ok(Err(CreateCommandError::Document(DocumentDbError::NotFound)));
        };
        let permission = tx
            .document_permission(workspace_id, actor_user_id, target_id, true)
            .await?;
        let current = tx.document_row(workspace_id, target_id).await?;
        if !permission_can_view(permission) || current.is_none() {
            return Ok(Err(CreateCommandError::Document(DocumentDbError::NotFound)));
        }
        let original: DocumentMeta = serde_json::from_value(original_result)
            .map_err(|err| sqlx::Error::Decode(Box::new(err)))?;
        if original.id != target_id || original.workspace_id != workspace_id {
            return Err(sqlx::Error::Protocol(
                "invalid wiki creation receipt target".into(),
            ));
        }
        return Ok(Ok(original));
    }
    let created = create_wiki_document_operation(
        tx,
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
        None,
    )
    .await?;
    let meta = match created {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            return Err(sqlx::Error::Protocol(
                "unfenced wiki create lost its target".into(),
            ))
        }
        Err(err) => return Ok(Err(CreateCommandError::Document(err))),
    };
    let result = serde_json::to_value(&meta).map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
    tx.insert_wiki_create_receipt(
        workspace_id,
        command_id,
        actor_user_id,
        &hash,
        meta.id,
        &result,
    )
    .await?;
    Ok(Ok(meta))
}

fn wiki_create_command_hash(
    workspace_id: Uuid,
    actor_user_id: Uuid,
    input: &CreateDocumentInput<'_>,
) -> Result<String, sqlx::Error> {
    // Only creation semantics are bound: omitted and null icons both create
    // SQL NULL. Fixed tuple encoding avoids map order and concatenation aliases.
    let bytes = serde_json::to_vec(&(
        "wiki-create-v1",
        workspace_id,
        actor_user_id,
        input.parent_id,
        input.title,
        input.icon.flatten(),
    ))
    .map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Lease of the import job a document is created for.
#[derive(Debug, Clone, Copy)]
pub struct ImportFence {
    pub job_id: Uuid,
    pub lease_token: Uuid,
}

/// Async import variant: the creator must still be a workspace admin at
/// execution time, and the document id is appended to the job's
/// `created_refs` in the same transaction (source `importTx`). `Ok(Ok(None))`
/// means the job's fence was lost and nothing was created.
pub async fn create_wiki_document_for_import(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    fence: ImportFence,
) -> Result<Result<Option<DocumentMeta>, DocumentDbError>, sqlx::Error> {
    create_wiki_document_inner(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        input,
        None,
        Some(fence),
    )
    .await
}

async fn create_wiki_document_inner(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
    fence: Option<ImportFence>,
) -> Result<Result<Option<DocumentMeta>, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = create_wiki_document_tx(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
        fence,
    )
    .await?;
    if matches!(result, Ok(Some(_))) {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

/// Reuses wiki creation and import fencing without taking commit ownership.
pub(crate) async fn create_wiki_document_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
    fence: Option<ImportFence>,
) -> Result<Result<Option<DocumentMeta>, DocumentDbError>, sqlx::Error> {
    create_wiki_document_operation(
        &mut OperationTx::Postgres(tx),
        workspace_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
        fence,
    )
    .await
}

pub(crate) async fn create_wiki_document_operation(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
    fence: Option<ImportFence>,
) -> Result<Result<Option<DocumentMeta>, DocumentDbError>, sqlx::Error> {
    if fence.is_some() && matches!(tx, OperationTx::SqliteFamily(_)) {
        return Err(sqlx::Error::Protocol(
            "SQLite-family import event deferral is pending W2 producer integration".into(),
        ));
    }
    let document_id = Uuid::now_v7();
    tx.lock_membership_users(&[actor_user_id]).await?;
    if !tx.recheck_session(actor_user_id, session_id).await? {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    tx.lock_tree(workspace_id).await?;
    if !tx.workspace_is_live(workspace_id).await? {
        return Ok(Err(DocumentDbError::NotFound));
    }
    let role = tx
        .membership_role(workspace_id, actor_user_id, true)
        .await?;
    if !wiki_can_edit(role) {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if fence.is_some() && !role.is_some_and(|r| r.at_least(WorkspaceRole::Admin)) {
        return Ok(Err(DocumentDbError::Forbidden));
    }

    let mut parent_path = String::new();
    if let Some(parent_id) = input.parent_id {
        let parent = tx.wiki_parent(workspace_id, parent_id).await?;
        let Some((project_id, path, deleted_at)) = parent else {
            return Ok(Err(DocumentDbError::NotFound));
        };
        if deleted_at.is_some() {
            return Ok(Err(DocumentDbError::NotFound));
        }
        if project_id.is_some() {
            return Ok(Err(DocumentDbError::AffiliationMismatch));
        }
        parent_path = path;
    }

    let depth = if input.parent_id.is_some() {
        depth_of(&parent_path) + 1
    } else {
        1
    };
    if depth > MAX_TREE_DEPTH {
        return Ok(Err(DocumentDbError::DepthLimit));
    }

    let last_sort = tx.last_wiki_sort_key(workspace_id, input.parent_id).await?;
    let sort_key = match between(last_sort.as_deref(), None) {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("{err}");
            return Ok(Err(DocumentDbError::InvalidSortKey));
        }
    };
    let path = if input.parent_id.is_some() {
        format!("{}.{}", parent_path, to_path_label(document_id))
    } else {
        to_path_label(document_id)
    };
    let icon = match input.icon {
        Some(value) => value.map(str::to_string),
        None => None,
    };
    let number = tx.allocate_wiki_number(workspace_id).await?;
    tx.insert_wiki_document(
        workspace_id,
        document_id,
        actor_user_id,
        input.title,
        icon.as_deref(),
        &path,
        input.parent_id,
        &sort_key,
        number,
    )
    .await?;

    let payload = json!({
        "documentId": document_id.to_string(),
        "parentId": input.parent_id.map(|id| id.to_string()),
        "title": input.title,
        "projectId": null,
    });
    tx.record_document_event_and_audit(
        workspace_id,
        actor_user_id,
        "document.created",
        document_id,
        payload,
        client_ip,
    )
    .await?;

    if let Some(fence) = fence {
        if !tx
            .append_import_document_ref(workspace_id, fence.job_id, fence.lease_token, document_id)
            .await?
        {
            return Ok(Ok(None));
        }
    }

    let row = tx.document_row(workspace_id, document_id).await?;
    match row {
        Some(row) => Ok(Ok(Some(row_to_meta(row, true)))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn get_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    get_wiki_document_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

pub async fn get_wiki_document_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    // Preserve the current PG route's READ COMMITTED transaction. Family reads
    // use one snapshot for current credential, affiliation, grants and body.
    let mut tx = match backend {
        Backend::Postgres(pool) => super::backend::DbTransaction::Postgres(pool.begin().await?),
        _ => backend.begin_read().await?,
    };
    let mut operation = tx.operation();
    operation.set_tenant(workspace_id).await?;
    if !operation.session_is_live(actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !operation.workspace_is_live(workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission = operation
        .document_permission(workspace_id, actor_user_id, document_id, true)
        .await?;
    if !permission_can_view(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let row = operation.document_row(workspace_id, document_id).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    match row {
        Some(row) if row.8.is_some() => Ok(Err(DocumentDbError::NotFound)),
        Some(row) => Ok(Ok(row_to_meta(row, false))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

/// Explicit cross-project wiki discovery, separate from the legacy wiki-only
/// tree. Permission, live rows and direct tag assignments share one snapshot.
pub async fn list_workspace_wiki_discovery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    tag: Option<Uuid>,
) -> Result<Result<Vec<TreeNode>, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let Some(role) = membership_role(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    };
    let guest_wiki = guest_wiki_document_ids_select_sql(1, 3);
    let visible_project = visible_project_sql("p", 2, 3);
    let rows = sqlx::query_as::<
        _,
        (
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
        ),
    >(&format!(
        r#"
        SELECT d.id, d.workspace_id, d.parent_id, d.project_id, d.title,
               d.icon, d.path, d.sort_key, d.number, d.status
        FROM fvoci.documents d
        WHERE d.workspace_id = $1 AND d.deleted_at IS NULL
          AND (
            (d.project_id IS NULL AND ($2 = false OR d.id IN ({guest_wiki})))
            OR EXISTS (
                SELECT 1 FROM fvoci.projects p
                WHERE p.workspace_id = d.workspace_id AND p.id = d.project_id
                  AND p.deleted_at IS NULL AND {visible_project}
            )
          )
          AND ($4::uuid IS NULL OR EXISTS (
              SELECT 1 FROM fvoci.document_tag_assignments a
              WHERE a.workspace_id = d.workspace_id AND a.document_id = d.id
                AND a.tag_id = $4
          ))
        ORDER BY d.sort_key COLLATE "C", d.id
        "#
    ))
    .bind(workspace_id)
    .bind(role == WorkspaceRole::Guest)
    .bind(actor_user_id)
    .bind(tag)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
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
            )| TreeNode {
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
        .collect()))
}

pub async fn list_wiki_tree(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TreeNode>, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    };
    if role == WorkspaceRole::Guest {
        tx.commit().await?;
        return Ok(Ok(Vec::new()));
    }
    let rows = sqlx::query_as::<
        _,
        (
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
        ),
    >(
        r#"
        SELECT id, workspace_id, parent_id, project_id, title, icon, path, sort_key, number, status
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NULL AND project_id IS NULL
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
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
            )| TreeNode {
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
        .collect()))
}

pub async fn list_wiki_ancestors(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<Vec<AncestorCrumb>, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, true).await?;
    if !permission_can_view(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let current = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if current.8.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, String, Option<Uuid>, i32)>(
        r#"
        WITH target AS (
            SELECT path FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
        )
        SELECT ancestor.id, ancestor.title, ancestor.icon, ancestor.path,
               ancestor.project_id, ancestor.number
        FROM fvoci.documents AS ancestor
        CROSS JOIN target
        WHERE ancestor.workspace_id = $1
          AND ancestor.id <> $2
          AND target.path IS NOT NULL
          AND (
            target.path = ancestor.path
            OR substr(target.path, 1, length(ancestor.path) + 1) = ancestor.path || '.'
          )
        ORDER BY (length(ancestor.path) - length(replace(ancestor.path, '.', '')))
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(
            |(id, title, icon, path, project_id, number)| AncestorCrumb {
                id,
                title,
                icon,
                path,
                project_id,
                number,
            },
        )
        .collect()))
}

pub async fn update_wiki_document_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    patch: UpdateDocumentMetaInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, true).await?;
    if !permission_can_edit(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let current: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, deleted_at)) = current else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || project_id.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    sqlx::query(
        r#"
        UPDATE fvoci.documents
        SET title = COALESCE($3, title),
            icon = CASE WHEN $4 THEN $5 ELSE icon END,
            status = COALESCE($6, status),
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(patch.title)
    .bind(patch.icon.is_some())
    .bind(patch.icon.flatten())
    .bind(patch.status)
    .execute(&mut *tx)
    .await?;

    let mut payload = json!({ "documentId": document_id.to_string() });
    if let Some(title) = patch.title {
        payload["title"] = json!(title);
    }
    if let Some(icon) = patch.icon {
        payload["icon"] = json!(icon);
    }
    if let Some(status) = patch.status {
        payload["status"] = json!(status);
    }
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.updated",
        document_id,
        payload,
        client_ip,
    )
    .await?;
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(row_to_meta(row, false))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub(crate) async fn fetch_document_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<DocumentRow>, sqlx::Error> {
    sqlx::query_as::<_, DocumentRow>(
        r#"
        SELECT id, workspace_id, title, number, icon, path, parent_id, sort_key, project_id,
               status, schema_version, version, created_by, created_at, updated_at, content_json
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

fn family_document_row(row: &FamilyRow) -> Result<DocumentRow, sqlx::Error> {
    Ok((
        row.cell(0)?.id()?,
        row.cell(1)?.id()?,
        row.cell(2)?.string()?,
        row.cell(3)?.int32()?,
        row.cell(4)?.optional(Cell::string)?,
        row.cell(5)?.string()?,
        row.cell(6)?.optional(Cell::id)?,
        row.cell(7)?.string()?,
        row.cell(8)?.optional(Cell::id)?,
        row.cell(9)?.string()?,
        row.cell(10)?.int32()?,
        row.cell(11)?.int32()?,
        row.cell(12)?.id()?,
        row.cell(13)?.datetime()?,
        row.cell(14)?.datetime()?,
        row.cell(15)?.value()?,
    ))
}

type WikiReceipt = (Uuid, String, Option<Uuid>, Value);
type WikiParent = (Option<Uuid>, String, Option<DateTime<Utc>>);

impl OperationTx<'_, '_> {
    pub(crate) async fn document_row(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<DocumentRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => fetch_document_row(tx, workspace, document).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT id,workspace_id,title,number,icon,path,parent_id,sort_key,project_id,status,schema_version,version,created_by,created_at,updated_at,content_json FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL", &[Cell::uuid(workspace),Cell::uuid(document)]).await?;
                rows.first().map(family_document_row).transpose()
            }
        }
    }

    async fn document_affiliation(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<(Option<Uuid>, Option<DateTime<Utc>>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT project_id,deleted_at FROM fvoci.documents WHERE workspace_id=$1 AND id=$2",
            )
            .bind(workspace)
            .bind(document)
            .fetch_optional(&mut ***tx)
            .await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT project_id,deleted_at FROM documents WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(workspace),Cell::uuid(document)]).await?;
                rows.first()
                    .map(|row| {
                        Ok((
                            row.cell(0)?.optional(Cell::id)?,
                            row.cell(1)?.optional(Cell::datetime)?,
                        ))
                    })
                    .transpose()
            }
        }
    }

    async fn wiki_group_roles(
        &mut self,
        workspace: Uuid,
        document: Uuid,
        user: Uuid,
    ) -> Result<Vec<String>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => wiki_group_roles(tx, workspace, document, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT dm.role FROM document_members dm JOIN group_members gm ON gm.workspace_id=dm.workspace_id AND gm.group_id=dm.group_id WHERE dm.workspace_id=?1 AND dm.document_id=?2 AND gm.user_id=?3 AND dm.group_id IS NOT NULL", &[Cell::uuid(workspace),Cell::uuid(document),Cell::uuid(user)]).await?;
                rows.iter().map(|row| row.cell(0)?.string()).collect()
            }
        }
    }

    async fn wiki_parent(
        &mut self,
        workspace: Uuid,
        parent: Uuid,
    ) -> Result<Option<WikiParent>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as("SELECT project_id,path::text,deleted_at FROM fvoci.documents WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(parent).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT project_id,path,deleted_at FROM documents WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(workspace),Cell::uuid(parent)]).await?;
                rows.first().map(|row|Ok((row.cell(0)?.optional(Cell::id)?,row.cell(1)?.string()?,row.cell(2)?.optional(Cell::datetime)?))).transpose()
            }
        }
    }

    async fn last_wiki_sort_key(
        &mut self,
        workspace: Uuid,
        parent: Option<Uuid>,
    ) -> Result<Option<String>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("SELECT sort_key FROM fvoci.documents WHERE workspace_id=$1 AND parent_id IS NOT DISTINCT FROM $2 AND deleted_at IS NULL ORDER BY sort_key COLLATE \"C\" DESC LIMIT 1").bind(workspace).bind(parent).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT sort_key FROM documents WHERE workspace_id=?1 AND parent_id IS ?2 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY DESC LIMIT 1", &[Cell::uuid(workspace),Cell::optional_uuid(parent)]).await?;
                rows.first().map(|row|row.cell(0)?.string()).transpose()
            }
        }
    }

    async fn allocate_wiki_number(&mut self, workspace: Uuid) -> Result<i32, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("UPDATE fvoci.workspaces SET next_document_number=next_document_number+1,updated_at=now() WHERE id=$1 AND deleted_at IS NULL RETURNING next_document_number").bind(workspace).fetch_one(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let rows=tx.query("UPDATE workspaces SET next_document_number=next_document_number+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND deleted_at IS NULL AND next_document_number<2147483647 RETURNING next_document_number", &[Cell::uuid(workspace)]).await?;
                rows.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.int32()
            }
        }
    }

    async fn insert_wiki_document(
        &mut self,
        workspace: Uuid,
        document: Uuid,
        actor: Uuid,
        title: &str,
        icon: Option<&str>,
        path: &str,
        parent: Option<Uuid>,
        sort: &str,
        number: i32,
    ) -> Result<(), sqlx::Error> {
        let body = empty_document_json();
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by,kind) VALUES($1,$2,$3,$4,$5,$6,$7,NULL,$8,'draft',$9,$10,$11,'doc')")
                    .bind(document).bind(workspace).bind(title).bind(icon).bind(path).bind(parent).bind(sort).bind(number).bind(DOCUMENT_SCHEMA_VERSION).bind(body).bind(actor).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute("INSERT INTO documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by,kind) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,?8,'draft',?9,?10,?11,'doc')", &[Cell::uuid(document),Cell::uuid(workspace),Cell::text(title),Cell::optional_text(icon),Cell::text(path),Cell::optional_uuid(parent),Cell::text(sort),Cell::Integer(i64::from(number)),Cell::Integer(i64::from(DOCUMENT_SCHEMA_VERSION)),Cell::json(&body)?,Cell::uuid(actor)]).await?;
            }
        }
        Ok(())
    }

    async fn wiki_create_receipt(
        &mut self,
        workspace: Uuid,
        command: Uuid,
    ) -> Result<Option<WikiReceipt>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as("SELECT actor_user_id,request_hash,document_id,result_json FROM fvoci.wiki_create_commands WHERE workspace_id=$1 AND command_id=$2").bind(workspace).bind(command).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT actor_user_id,request_hash,document_id,result_json FROM wiki_create_commands WHERE workspace_id=?1 AND command_id=?2", &[Cell::uuid(workspace),Cell::uuid(command)]).await?;
                rows.first().map(|row|Ok((row.cell(0)?.id()?,row.cell(1)?.string()?,row.cell(2)?.optional(Cell::id)?,row.cell(3)?.value()?))).transpose()
            }
        }
    }

    async fn insert_wiki_create_receipt(
        &mut self,
        workspace: Uuid,
        command: Uuid,
        actor: Uuid,
        hash: &str,
        document: Uuid,
        result: &Value,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.wiki_create_commands(workspace_id,command_id,actor_user_id,request_hash,document_id,result_json) VALUES($1,$2,$3,$4,$5,$6)").bind(workspace).bind(command).bind(actor).bind(hash).bind(document).bind(result).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute("INSERT INTO wiki_create_commands(workspace_id,command_id,actor_user_id,request_hash,document_id,result_json) VALUES(?1,?2,?3,?4,?5,?6)", &[Cell::uuid(workspace),Cell::uuid(command),Cell::uuid(actor),Cell::text(hash),Cell::uuid(document),Cell::json(result)?]).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_document_event_and_audit(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        verb: &str,
        target: Uuid,
        payload: Value,
        ip: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        self.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: verb.into(),
            target_type: Some("document".into()),
            target_id: Some(target),
            payload: payload.clone(),
        })
        .await?;
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: verb.into(),
            target_type: Some("document".into()),
            target_id: Some(target),
            payload,
            ip: ip.map(str::to_string),
        })
        .await
    }

    async fn append_import_document_ref(
        &mut self,
        workspace: Uuid,
        job: Uuid,
        owner: Uuid,
        document: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                crate::db::import_jobs::append_import_document_ref(
                    tx, workspace, job, owner, document,
                )
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                Err(sqlx::Error::Protocol(
                    "SQLite-family import progress is pending W2 producer integration".into(),
                ))
            }
        }
    }
}

fn alphabet_char(index: usize) -> char {
    char::from(FRACTIONAL_ALPHABET[index])
}

fn digit_at(s: &str, i: usize) -> usize {
    s.as_bytes()
        .get(i)
        .and_then(|ch| FRACTIONAL_ALPHABET.iter().position(|item| item == ch))
        .unwrap_or(0)
}

fn is_canonical_key(s: &str) -> bool {
    if s.is_empty() || s.as_bytes().last() == Some(&b'0') {
        return false;
    }
    s.bytes().all(|ch| FRACTIONAL_ALPHABET.contains(&ch))
}

fn key_after(s: &str) -> String {
    if s.is_empty() {
        return alphabet_char(FRACTIONAL_MID).to_string();
    }
    let last = digit_at(s, s.len() - 1);
    if last < FRACTIONAL_BASE - 1 {
        let mut out = s[..s.len() - 1].to_string();
        out.push(alphabet_char(last + 1));
        out
    } else {
        let mut out = s.to_string();
        out.push(alphabet_char(FRACTIONAL_MID));
        out
    }
}

fn midpoint(a: &str, b: &str) -> String {
    let mut i = 0;
    let mut digits = String::new();
    loop {
        let da = digit_at(a, i);
        let db = digit_at(b, i);
        if da == db {
            digits.push(alphabet_char(da));
            i += 1;
            continue;
        }
        if db.saturating_sub(da) >= 2 {
            digits.push(alphabet_char((da + db) / 2));
            return digits;
        }
        digits.push(alphabet_char(da));
        digits.push_str(&key_after(a.get(i + 1..).unwrap_or("")));
        return digits;
    }
}

pub(crate) async fn subtree_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    root_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        WITH root AS (
            SELECT path FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
        )
        SELECT d.id
        FROM fvoci.documents AS d
        CROSS JOIN root
        WHERE d.workspace_id = $1
          AND root.path IS NOT NULL
          AND (
            d.path = root.path
            OR substr(d.path, 1, length(root.path) + 1) = root.path || '.'
          )
        "#,
    )
    .bind(workspace_id)
    .bind(root_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub(crate) async fn is_descendant(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    ancestor_id: Uuid,
    descendant_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> = sqlx::query_as(
        r#"
        WITH a_doc AS (
            SELECT path FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
        ),
        b_doc AS (
            SELECT path FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $3
        )
        SELECT (
            a_doc.path IS NOT NULL
            AND b_doc.path IS NOT NULL
            AND (
                a_doc.path = b_doc.path
                OR substr(a_doc.path, 1, length(b_doc.path) + 1) = b_doc.path || '.'
            )
        )
        FROM a_doc
        CROSS JOIN b_doc
        "#,
    )
    .bind(workspace_id)
    .bind(ancestor_id)
    .bind(descendant_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(v,)| v).unwrap_or(false))
}

pub(crate) async fn lock_document_rows(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    if document_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        SELECT id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = ANY($2)
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub fn resolve_reorder_sort_key(
    siblings: &[TreeNode],
    document_id: Uuid,
    after_id: Option<Uuid>,
) -> Result<String, DocumentDbError> {
    let filtered = siblings
        .iter()
        .filter(|node| node.id != document_id)
        .collect::<Vec<_>>();
    if let Some(after_id) = after_id {
        let idx = filtered.iter().position(|node| node.id == after_id);
        if idx.is_none() {
            return Err(DocumentDbError::NotFound);
        }
        let idx = idx.unwrap();
        let left_key = filtered[idx].sort_key.as_str();
        let right_key = filtered.get(idx + 1).map(|node| node.sort_key.as_str());
        return between(Some(left_key), right_key).map_err(|_| DocumentDbError::InvalidSortKey);
    }
    let first_key = filtered.first().map(|node| node.sort_key.as_str());
    between(None, first_key).map_err(|_| DocumentDbError::InvalidSortKey)
}

pub(crate) async fn list_live_siblings(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<Vec<TreeNode>, sqlx::Error> {
    list_live_siblings_in(tx, workspace_id, None, parent_id).await
}

/// Live children of `parent_id` in the wiki (`project_id = None`) or one project.
pub(crate) async fn list_live_siblings_in(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
) -> Result<Vec<TreeNode>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
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
        ),
    >(
        r#"
        SELECT id, workspace_id, parent_id, project_id, title, icon, path, sort_key, number, status
        FROM fvoci.documents
        WHERE workspace_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND deleted_at IS NULL
          AND project_id IS NOT DISTINCT FROM $3
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .bind(project_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
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
            )| TreeNode {
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
        .collect())
}

async fn renumber_subtree_for_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    document_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    for document_id in document_ids {
        let number: (i32,) = sqlx::query_as(
            r#"
            UPDATE fvoci.projects
            SET next_number = next_number + 1, updated_at = now()
            WHERE workspace_id = $1 AND id = $2
            RETURNING next_number - 1
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut **tx)
        .await?;
        // Number and affiliation form one unique namespace. Applying the new
        // number while still in the wiki can collide with another subtree row.
        // The tree/project locks and transaction cover the subsequent path move.
        sqlx::query(
            r#"
            UPDATE fvoci.documents
            SET number = $3, project_id = $4, updated_at = now()
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(document_id)
        .bind(number.0)
        .bind(project_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) async fn move_subtree(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    new_parent_id: Option<Uuid>,
    new_path: &str,
    new_project_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        WITH old AS (
            SELECT path FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
        )
        UPDATE fvoci.documents AS d
        SET path = CASE
                WHEN d.id = $2 THEN $4
                ELSE $4 || substr(d.path, length(old.path) + 1)
            END,
            parent_id = CASE WHEN d.id = $2 THEN $3 ELSE d.parent_id END,
            project_id = $5,
            updated_at = now()
        FROM old
        WHERE d.workspace_id = $1
          AND (
            d.path = old.path
            OR substr(d.path, 1, length(old.path) + 1) = old.path || '.'
          )
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(new_parent_id)
    .bind(new_path)
    .bind(new_project_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn trash_document_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE fvoci.documents
        SET deleted_at = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn move_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    new_parent_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, true).await?;
    if !permission_can_edit(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let subtree = subtree_ids(&mut tx, workspace_id, document_id).await?;
    lock_document_rows(&mut tx, workspace_id, &subtree).await?;

    let doc: Option<DocProjectDeletedPathParent> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at, path, parent_id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((doc_project_id, deleted_at, doc_path, old_parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || doc_project_id.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let parent: Option<(Option<Uuid>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at, path
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(new_parent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((new_parent_project_id, parent_deleted_at, parent_path)) = parent else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if parent_deleted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    if let Some(project_id) = new_parent_project_id {
        if doc_project_id != Some(project_id) {
            let locked = lock_project(&mut tx, workspace_id, project_id).await?;
            let Some(locked) = locked else {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            };
            if locked.status == "archived" {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            }
            let permission =
                project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
            if !permission.at_least(ProjectPermission::Edit) {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            }
        }
    }

    if is_descendant(&mut tx, workspace_id, new_parent_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Cycle));
    }

    let own_depth = depth_of(&doc_path);
    let new_depth = depth_of(&parent_path) + 1;
    let mut max_relative_depth = 0i32;
    for id in &subtree {
        if *id == document_id {
            continue;
        }
        let row: Option<(String,)> =
            sqlx::query_as("SELECT path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2")
                .bind(workspace_id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((path,)) = row {
            max_relative_depth = max_relative_depth.max(depth_of(&path) - own_depth);
        }
    }
    if new_depth + max_relative_depth > MAX_TREE_DEPTH {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::DepthLimit));
    }

    let new_path = format!("{}.{}", parent_path, to_path_label(document_id));
    let dest_project_id = new_parent_project_id;
    if doc_project_id.is_none() {
        if let Some(project_id) = dest_project_id {
            renumber_subtree_for_project(&mut tx, workspace_id, project_id, &subtree).await?;
        }
    }
    move_subtree(
        &mut tx,
        workspace_id,
        document_id,
        Some(new_parent_id),
        &new_path,
        dest_project_id,
    )
    .await?;

    let payload = json!({
        "documentId": document_id.to_string(),
        "newParentId": new_parent_id.to_string(),
        "newPath": new_path,
        "oldParentId": old_parent_id.map(|id| id.to_string()),
        "oldPath": doc_path,
        "oldProjectId": doc_project_id.map(|id| id.to_string()),
        "newProjectId": dest_project_id.map(|id| id.to_string()),
    });
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.moved",
        document_id,
        payload,
        client_ip,
    )
    .await?;

    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(row_to_meta(row, false))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn reorder_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    after_id: Option<Uuid>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, true).await?;
    if !permission_can_edit(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let current: Option<DocParentProjectDeleted> = sqlx::query_as(
        r#"
        SELECT parent_id, project_id, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((parent_id, project_id, deleted_at)) = current else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || project_id.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let siblings = list_live_siblings(&mut tx, workspace_id, parent_id).await?;
    let new_sort_key = match resolve_reorder_sort_key(&siblings, document_id, after_id) {
        Ok(key) => key,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    sqlx::query(
        r#"
        UPDATE fvoci.documents
        SET sort_key = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(&new_sort_key)
    .execute(&mut *tx)
    .await?;

    let payload = json!({
        "documentId": document_id.to_string(),
        "kind": "reorder",
        "afterId": after_id.map(|id| id.to_string()),
        "newSortKey": new_sort_key,
    });
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.moved",
        document_id,
        payload,
        client_ip,
    )
    .await?;

    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(row_to_meta(row, false))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn trash_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    children: TrashChildrenMode,
    client_ip: Option<&str>,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    let now = Utc::now();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, true).await?;
    if !permission_can_edit(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let doc: Option<DocProjectDeletedParent> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at, parent_id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, deleted_at, parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || project_id.is_some() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    if children == TrashChildrenMode::Reparent {
        let direct_children = list_live_siblings(&mut tx, workspace_id, Some(document_id)).await?;
        let mut parent_path = String::new();
        let dest_project_id: Option<Uuid> = if let Some(grandparent_id) = parent_id {
            let parent: Option<(Option<Uuid>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
                r#"
                SELECT project_id, deleted_at, path
                FROM fvoci.documents
                WHERE workspace_id = $1 AND id = $2
                "#,
            )
            .bind(workspace_id)
            .bind(grandparent_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((gp_project_id, gp_deleted_at, gp_path)) = parent else {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            };
            if gp_deleted_at.is_some() {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            }
            parent_path = gp_path;
            gp_project_id
        } else {
            None
        };

        let dest_siblings = list_live_siblings(&mut tx, workspace_id, parent_id).await?;
        let mut last_key = dest_siblings
            .iter()
            .rev()
            .find(|node| node.id != document_id)
            .map(|node| node.sort_key.clone());

        for child in direct_children {
            let child_subtree = subtree_ids(&mut tx, workspace_id, child.id).await?;
            lock_document_rows(&mut tx, workspace_id, &child_subtree).await?;
            let child_row: Option<(String, Option<Uuid>)> = sqlx::query_as(
                "SELECT path, parent_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(child.id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((child_path, child_parent_id)) = child_row else {
                continue;
            };
            let new_path = if parent_id.is_some() {
                format!("{}.{}", parent_path, to_path_label(child.id))
            } else {
                to_path_label(child.id)
            };
            move_subtree(
                &mut tx,
                workspace_id,
                child.id,
                parent_id,
                &new_path,
                dest_project_id,
            )
            .await?;
            let sort_key = match between(last_key.as_deref(), None) {
                Ok(key) => key,
                Err(_) => {
                    tx.rollback().await?;
                    return Ok(Err(DocumentDbError::InvalidSortKey));
                }
            };
            sqlx::query(
                r#"
                UPDATE fvoci.documents
                SET sort_key = $3, updated_at = now()
                WHERE workspace_id = $1 AND id = $2
                "#,
            )
            .bind(workspace_id)
            .bind(child.id)
            .bind(&sort_key)
            .execute(&mut *tx)
            .await?;
            last_key = Some(sort_key);
            let payload = json!({
                "documentId": child.id.to_string(),
                "newParentId": parent_id.map(|id| id.to_string()),
                "newPath": new_path,
                "oldParentId": child_parent_id.map(|id| id.to_string()),
                "oldPath": child_path,
                "oldProjectId": null,
                "newProjectId": dest_project_id.map(|id| id.to_string()),
            });
            record_document_event_and_audit(
                &mut tx,
                workspace_id,
                actor_user_id,
                "document.moved",
                child.id,
                payload,
                client_ip,
            )
            .await?;
        }

        lock_document_rows(&mut tx, workspace_id, &[document_id]).await?;
        if !trash_document_row(&mut tx, workspace_id, document_id, now).await? {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        }
        record_document_event_and_audit(
            &mut tx,
            workspace_id,
            actor_user_id,
            "document.trashed",
            document_id,
            json!({ "documentId": document_id.to_string() }),
            client_ip,
        )
        .await?;
        tx.commit().await?;
        return Ok(Ok(()));
    }

    let subtree = subtree_ids(&mut tx, workspace_id, document_id).await?;
    lock_document_rows(&mut tx, workspace_id, &subtree).await?;
    for id in subtree {
        let live: Option<(Option<DateTime<Utc>>, Option<Uuid>)> = sqlx::query_as(
            "SELECT deleted_at, project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((deleted_at, project_id)) = live else {
            continue;
        };
        if deleted_at.is_some() || project_id.is_some() {
            continue;
        }
        if !trash_document_row(&mut tx, workspace_id, id, now).await? {
            continue;
        }
        record_document_event_and_audit(
            &mut tx,
            workspace_id,
            actor_user_id,
            "document.trashed",
            id,
            json!({ "documentId": id.to_string() }),
            client_ip,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn list_trashed_wiki_documents(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TrashNode>, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if role.is_none() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    let rows = sqlx::query_as::<_, (Uuid, String, DateTime<Utc>, Option<Uuid>)>(
        r#"
        SELECT id, title, deleted_at, project_id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NOT NULL
          AND deleted_at > now() - make_interval(days => $2)
        ORDER BY deleted_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(TRASH_RETENTION_DAYS)
    .fetch_all(&mut *tx)
    .await?;

    // Source `listTrashedDocuments`: wiki rows by document permission, project
    // rows by the live project's permission (a trashed project hides its rows).
    let mut project_permissions: std::collections::HashMap<Uuid, Option<ProjectPermission>> =
        std::collections::HashMap::new();
    let mut visible = Vec::new();
    for (id, title, deleted_at, project_id) in rows {
        let permission = match project_id {
            Some(project_id) => match project_permissions.get(&project_id) {
                Some(cached) => cached.unwrap_or(ProjectPermission::None),
                None => {
                    let permission = crate::db::projects::project_permission_by_id(
                        &mut tx,
                        workspace_id,
                        actor_user_id,
                        project_id,
                    )
                    .await?;
                    project_permissions.insert(project_id, permission);
                    permission.unwrap_or(ProjectPermission::None)
                }
            },
            None => document_permission(&mut tx, workspace_id, actor_user_id, id, false).await?,
        };
        if permission_can_view(permission) {
            visible.push(TrashNode {
                id,
                title,
                deleted_at,
                project_id,
            });
        }
    }
    tx.commit().await?;
    Ok(Ok(visible))
}

pub async fn restore_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }

    lock_document_rows(&mut tx, workspace_id, &[document_id]).await?;
    let doc: Option<DocProjectDeletedParent> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at, parent_id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, deleted_at, parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let expired = match deleted_at {
        Some(at) => trash_expired(&mut tx, at).await?,
        None => false,
    };
    if deleted_at.is_none() || project_id.is_some() || expired {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission =
        document_permission(&mut tx, workspace_id, actor_user_id, document_id, false).await?;
    if !permission_can_edit(permission) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    if let Some(parent_id) = parent_id {
        let parent: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
            "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((parent_deleted_at,)) = parent else {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::TrashedParent));
        };
        if parent_deleted_at.is_some() {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::TrashedParent));
        }
    }

    let restored = sqlx::query(
        r#"
        UPDATE fvoci.documents
        SET deleted_at = NULL, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    if restored.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.restored",
        document_id,
        json!({ "documentId": document_id.to_string() }),
        client_ip,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub fn between(a: Option<&str>, b: Option<&str>) -> Result<String, FractionalError> {
    if let Some(a) = a {
        if !is_canonical_key(a) {
            return Err(FractionalError::InvalidKey {
                side: "a",
                key: a.to_string(),
            });
        }
    }
    if let Some(b) = b {
        if !is_canonical_key(b) {
            return Err(FractionalError::InvalidKey {
                side: "b",
                key: b.to_string(),
            });
        }
    }
    match (a, b) {
        (Some(a), Some(b)) => {
            if a >= b {
                return Err(FractionalError::OutOfOrder {
                    a: a.to_string(),
                    b: b.to_string(),
                });
            }
            Ok(midpoint(a, b))
        }
        (_, None) => Ok(key_after(a.unwrap_or(""))),
        (None, Some(b)) => Ok(midpoint("", b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sort_key_matches_source_midpoint() {
        let first = between(None, None).unwrap();
        let before = between(None, Some(&first)).unwrap();
        let after = between(Some(&first), None).unwrap();
        assert_eq!(first, "V");
        assert_eq!(before, "F");
        assert_eq!(after, "W");
        assert!(before < first);
        assert!(first < after);
        let mut key = first.clone();
        let mut keys = vec![key.clone()];
        for _ in 0..10 {
            key = between(Some(&key), None).unwrap();
            keys.push(key.clone());
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(sorted, keys);
    }

    #[test]
    fn midpoint_matches_source_when_left_is_shorter_than_divergence() {
        assert_eq!(between(None, Some("1")).unwrap(), "0V");
        assert_eq!(between(Some("V"), Some("V1")).unwrap(), "V0V");
    }

    #[test]
    fn between_returns_error_for_equal_and_noncanonical_keys() {
        assert_eq!(
            between(Some("V"), Some("V")),
            Err(FractionalError::OutOfOrder {
                a: "V".to_string(),
                b: "V".to_string(),
            })
        );
        assert_eq!(
            between(Some("A0"), None),
            Err(FractionalError::InvalidKey {
                side: "a",
                key: "A0".to_string(),
            })
        );
        assert_eq!(
            between(Some(""), None),
            Err(FractionalError::InvalidKey {
                side: "a",
                key: String::new(),
            })
        );
        assert_eq!(
            between(Some("A!"), None),
            Err(FractionalError::InvalidKey {
                side: "a",
                key: "A!".to_string(),
            })
        );
    }

    #[test]
    fn path_label_strips_uuid_dashes() {
        let id = Uuid::parse_str("0199a1c2-3b4d-7e8f-9012-3456789abcde").unwrap();
        assert_eq!(to_path_label(id), "0199a1c23b4d7e8f90123456789abcde");
        assert_eq!(format_display_id("WIKI", 12), "WIKI-12");
    }

    #[test]
    fn empty_json_is_canonical_tiptap_seed() {
        assert_eq!(
            empty_document_json(),
            json!({"type":"doc","content":[{"type":"paragraph"}]})
        );
    }
}
