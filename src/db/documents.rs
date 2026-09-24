use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_key_from_uuid, set_tenant};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::workspace::WorkspaceRole;

pub(crate) const MEMBERSHIP_LOCK_NAMESPACE: i32 = 1_907_006;
const TREE_LOCK_NAMESPACE: i32 = 1_907_005;
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

#[derive(Debug, Clone)]
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

fn depth_of(path: &str) -> i32 {
    path.split('.').count() as i32
}

pub(crate) fn wiki_can_edit(role: Option<WorkspaceRole>) -> bool {
    matches!(
        role,
        Some(WorkspaceRole::Owner | WorkspaceRole::Admin | WorkspaceRole::Member)
    )
}

fn wiki_can_view(role: Option<WorkspaceRole>) -> bool {
    wiki_can_edit(role)
}

pub(crate) async fn lock_membership_users(
    tx: &mut Transaction<'_, Postgres>,
    user_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut keys = user_ids
        .iter()
        .map(|id| lock_key_from_uuid(*id))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
            .bind(MEMBERSHIP_LOCK_NAMESPACE)
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn lock_tree(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TREE_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(workspace_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn recheck_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (
            s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
            AND u.deleted_at IS NULL
            AND u.suspended_at IS NULL
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        FOR UPDATE OF u, s
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

pub(crate) async fn session_is_live(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (
            s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
            AND u.deleted_at IS NULL
            AND u.suspended_at IS NULL
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

pub(crate) async fn membership_role(
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

pub(crate) async fn membership_role_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

pub async fn workspace_is_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(live,)| live).unwrap_or(false))
}

async fn record_document_event_and_audit(
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

fn row_to_meta(row: DocumentRow, with_display_id: bool) -> DocumentMeta {
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
    let document_id = Uuid::now_v7();
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
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }

    let mut parent_path = String::new();
    if let Some(parent_id) = input.parent_id {
        let parent: Option<(Option<Uuid>, String, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            SELECT project_id, path, deleted_at
            FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((project_id, path, deleted_at)) = parent else {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        };
        if deleted_at.is_some() {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        }
        if project_id.is_some() {
            tx.rollback().await?;
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
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::DepthLimit));
    }

    let last_sort: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT sort_key
        FROM fvoci.documents
        WHERE workspace_id = $1
          AND parent_id IS NOT DISTINCT FROM $2
          AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C" DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(input.parent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let sort_key = match between(last_sort.as_ref().map(|(k,)| k.as_str()), None) {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("{err}");
            tx.rollback().await?;
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
    let number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.workspaces
        SET next_document_number = next_document_number + 1, updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING next_document_number
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, icon, path, parent_id, sort_key, project_id,
            number, status, schema_version, content_json, created_by, kind
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, NULL,
            $8, 'draft', $9, $10, $11, 'doc'
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(input.title)
    .bind(icon)
    .bind(&path)
    .bind(input.parent_id)
    .bind(&sort_key)
    .bind(number.0)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    let payload = json!({
        "documentId": document_id.to_string(),
        "parentId": input.parent_id.map(|id| id.to_string()),
        "title": input.title,
        "projectId": null,
    });
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.created",
        document_id,
        payload,
        client_ip,
    )
    .await?;

    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(row_to_meta(row, true))),
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
    if !wiki_can_view(role) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) if row.8.is_some() => Ok(Err(DocumentDbError::NotFound)),
        Some(row) => Ok(Ok(row_to_meta(row, false))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
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
        WHERE workspace_id = $1 AND deleted_at IS NULL
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
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !wiki_can_view(role) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
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
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
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

async fn fetch_document_row(
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
