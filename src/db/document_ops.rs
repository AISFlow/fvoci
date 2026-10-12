//! Document routes shared by wiki and project documents: access checks for body
//! writes, children, project ancestors, backlinks and duplicate.
//!
//! Source: `packages/core/src/document.ts`, `reference.ts`, `document-duplicate.ts`
//! at `393795261322b916e588043cf94feca999175843`.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::codec::Cell;
use crate::db::collab::COLLAB_STATE_ENCODING_V1;
use crate::db::context::{
    begin_read, lock_membership_users, lock_tree, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{
    between, document_permission, fetch_document_row, format_display_id,
    record_document_event_and_audit, row_to_meta, to_path_label, AncestorCrumb, DocumentDbError,
    DocumentMeta, DOCUMENT_SCHEMA_VERSION,
};
use crate::db::project_documents::{
    assert_project_document, project_key, require_project_document_access, with_project_display_id,
};
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;

/// Which route family addressed the document (source `affiliationFromParams`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentScope {
    Wiki,
    Project(Uuid),
}

impl DocumentScope {
    pub fn project_id(self) -> Option<Uuid> {
        match self {
            Self::Wiki => None,
            Self::Project(id) => Some(id),
        }
    }
}

/// Session, workspace, route affiliation and permission of one live document.
/// Wiki documents use `document_permission`; project documents the project's
/// effective permission (an archived project refuses edit). `lock` takes the
/// project row lock for a caller that writes in this transaction; see
/// `require_project_document_access`.
async fn scoped_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    min: ProjectPermission,
    lock: bool,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    match scope {
        DocumentScope::Wiki => {
            if !session_is_live(tx, actor_user_id, session_id).await? {
                return Ok(Err(DocumentDbError::Forbidden));
            }
            if !workspace_is_live(tx, workspace_id).await? {
                return Ok(Err(DocumentDbError::NotFound));
            }
            // `document_permission` is `None` for project and trashed documents.
            let permission =
                document_permission(tx, workspace_id, actor_user_id, document_id, true).await?;
            if !permission.at_least(min) {
                return Ok(Err(DocumentDbError::NotFound));
            }
            Ok(Ok(()))
        }
        DocumentScope::Project(project_id) => {
            if let Err(err) = require_project_document_access(
                tx,
                workspace_id,
                actor_user_id,
                session_id,
                project_id,
                min,
                lock,
            )
            .await?
            {
                return Ok(Err(err));
            }
            assert_project_document(
                tx,
                workspace_id,
                project_id,
                document_id,
                true,
                DocumentDbError::NotFound,
            )
            .await
        }
    }
}

async fn scoped_meta(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
) -> Result<Option<DocumentMeta>, sqlx::Error> {
    let Some(row) = fetch_document_row(tx, workspace_id, document_id).await? else {
        return Ok(None);
    };
    match scope {
        DocumentScope::Wiki => Ok(Some(row_to_meta(row, false))),
        DocumentScope::Project(project_id) => {
            let Some(key) = project_key(tx, workspace_id, project_id).await? else {
                return Ok(None);
            };
            Ok(Some(with_project_display_id(row_to_meta(row, true), &key)))
        }
    }
}

/// Current metadata when the actor holds at least `min` on the document. A
/// read-only check: the transaction ends before the caller acts, so it takes
/// no project row lock (a body write rechecks inside its own write).
pub async fn authorize_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    min: ProjectPermission,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    authorize_document_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        actor_user_id,
        session_id,
        scope,
        document_id,
        min,
    )
    .await
}

/// One current read snapshot for the ON body route's preliminary and response
/// authorization. The native room performs its own authoritative write proof.
pub async fn authorize_document_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    min: ProjectPermission,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let mut operation = tx.operation();
    operation.set_tenant(workspace_id).await?;
    let result = operation
        .authorize_document_current_scope(
            (workspace_id, document_id),
            actor_user_id,
            session_id,
            scope,
            min,
        )
        .await?;
    if result.is_err() {
        tx.rollback().await?;
    } else {
        tx.commit().await.map_err(|unknown| unknown.source)?;
    }
    Ok(result)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn authorize_document_current_scope(
        &mut self,
        target: (Uuid, Uuid),
        actor: Uuid,
        credential: Uuid,
        scope: DocumentScope,
        min: ProjectPermission,
    ) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
        let (workspace, document) = target;
        if !self.session_is_live(actor, credential).await? {
            return Ok(Err(DocumentDbError::Forbidden));
        }
        if !self.workspace_is_live(workspace).await? {
            return Ok(Err(DocumentDbError::NotFound));
        }
        let project_key = match scope {
            DocumentScope::Wiki => {
                if !self
                    .document_permission(workspace, actor, document, true)
                    .await?
                    .at_least(min)
                {
                    return Ok(Err(DocumentDbError::NotFound));
                }
                None
            }
            DocumentScope::Project(project) => {
                let Some((key, archived, permission)) = self
                    .document_scope_project(workspace, actor, project)
                    .await?
                else {
                    return Ok(Err(DocumentDbError::NotFound));
                };
                if (archived && min >= ProjectPermission::Edit) || !permission.at_least(min) {
                    return Ok(Err(DocumentDbError::NotFound));
                }
                Some(key)
            }
        };
        let Some(row) = self.document_row(workspace, document).await? else {
            return Ok(Err(DocumentDbError::NotFound));
        };
        if row.8 != scope.project_id() {
            return Ok(Err(DocumentDbError::NotFound));
        }
        let meta = row_to_meta(row, scope.project_id().is_some());
        Ok(Ok(match project_key {
            Some(key) => with_project_display_id(meta, &key),
            None => meta,
        }))
    }

    async fn document_scope_project(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        project: Uuid,
    ) -> Result<Option<(String, bool, ProjectPermission)>, sqlx::Error> {
        let details = match self {
            Self::Postgres(tx) => crate::db::projects::load_live_project(tx, workspace, project)
                .await?
                .map(|row| (row.key, row.status == "archived")),
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                tx.query(
                    "SELECT key,status FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                    &[Cell::uuid(workspace), Cell::uuid(project)],
                )
                .await?
                .first()
                .map(|row| Ok::<_, sqlx::Error>((row.cell(0)?.string()?, row.cell(1)?.string()? == "archived")))
                .transpose()?
            }
        };
        let Some((key, archived)) = details else {
            return Ok(None);
        };
        Ok(self
            .project_permission_by_id(workspace, actor, project)
            .await?
            .map(|permission| (key, archived, permission)))
    }
}

/// Source `listDocumentAncestors` for a project document.
pub async fn list_project_ancestors(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<Vec<AncestorCrumb>, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = scoped_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        DocumentScope::Project(project_id),
        document_id,
        ProjectPermission::View,
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
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
          AND substr(target.path, 1, length(ancestor.path) + 1) = ancestor.path || '.'
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

pub async fn list_project_ancestors_backend(
    backend: &Backend,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<Vec<AncestorCrumb>, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_project_ancestors(
            pool,
            workspace_id,
            project_id,
            actor_user_id,
            session_id,
            document_id,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        let meta = match op.authorize_document_current_scope((workspace_id, document_id), actor_user_id, session_id, DocumentScope::Project(project_id), ProjectPermission::View).await? {
            Ok(meta) => meta,
            Err(e) => return Ok(Err(e)),
        };
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        let rows = family
            .query(
                r#"
SELECT id,title,icon,path,project_id,number FROM documents WHERE workspace_id=?1 AND id<>?2 AND substr(?3,1,length(path)+1)=path||'.' ORDER BY length(path)-length(replace(path,'.',''))
"#,
                &[Cell::uuid(workspace_id), Cell::uuid(document_id), Cell::text(meta.path)],
            )
            .await?;
        Ok(Ok(rows.iter().map(|row| Ok(AncestorCrumb { id: row.cell(0)?.id()?, title: row.cell(1)?.string()?, icon: row.cell(2)?.optional(Cell::string)?, path: row.cell(3)?.string()?, project_id: row.cell(4)?.optional(Cell::id)?, number: row.cell(5)?.int32()? })).collect::<Result<Vec<_>, sqlx::Error>>()?))
    }
    .await;
    finish_document_operation(tx, result).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacklinkKind {
    Document,
    Task,
}

impl BacklinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Task => "task",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Backlink {
    pub kind: BacklinkKind,
    pub id: Uuid,
    pub title: String,
    pub display_id: Option<String>,
}

/// SQL/JSONPath form of source `extractInternalRefs` for one target: a
/// `mention` (`attrs.id`) or `embed` (`attrs.ref`) whose `attrs.entity` is the
/// target kind. UUID text compares case-insensitively like the source's uuid column.
fn internal_ref_jsonpath(entity: &str, target_id: Uuid) -> String {
    // Canonical hyphenated UUID and fixed entity names only: no user text reaches the path.
    let id = target_id.hyphenated().to_string();
    format!(
        r#"$.** ? ((@.type == "mention" && @.attrs.entity == "{entity}" && @.attrs.id like_regex "^{id}$" flag "i") || (@.type == "embed" && @.attrs.entity == "{entity}" && @.attrs.ref like_regex "^{id}$" flag "i"))"#
    )
}

/// Source `listDocumentBacklinks`: every live document or task whose current
/// body references this document, filtered by the actor's view permission on
/// the referencing item. References are derived from the stored bodies at read
/// time instead of a separately maintained table.
pub async fn list_document_backlinks(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
) -> Result<Result<Vec<Backlink>, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    // Bounds the two body scans (JSONPath over every live document and task).
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = scoped_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        scope,
        document_id,
        ProjectPermission::View,
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let path = internal_ref_jsonpath("document", document_id);
    let docs = sqlx::query_as::<_, (Uuid, String, Option<Uuid>, i32, Option<String>)>(
        r#"
        SELECT d.id, d.title, d.project_id, d.number, p.key
        FROM fvoci.documents d
        LEFT JOIN fvoci.projects p
          ON p.workspace_id = d.workspace_id AND p.id = d.project_id AND p.deleted_at IS NULL
        WHERE d.workspace_id = $1
          AND d.id <> $2
          AND d.deleted_at IS NULL
          AND jsonb_path_exists(d.content_json, $3::jsonpath)
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(&path)
    .fetch_all(&mut *tx)
    .await?;
    let tasks = sqlx::query_as::<_, (Uuid, String, Uuid, i32, Option<String>)>(
        r#"
        SELECT t.id, t.title, t.project_id, t.number, p.key
        FROM fvoci.tasks t
        LEFT JOIN fvoci.projects p
          ON p.workspace_id = t.workspace_id AND p.id = t.project_id AND p.deleted_at IS NULL
        WHERE t.workspace_id = $1
          AND t.deleted_at IS NULL
          AND jsonb_path_exists(t.content_json, $2::jsonpath)
        "#,
    )
    .bind(workspace_id)
    .bind(&path)
    .fetch_all(&mut *tx)
    .await?;

    let mut project_views: HashMap<Uuid, bool> = HashMap::new();
    let mut items = Vec::new();
    for (id, title, project_id, number, key) in docs {
        let visible = match project_id {
            None => document_permission(&mut tx, workspace_id, actor_user_id, id, true)
                .await?
                .at_least(ProjectPermission::View),
            Some(project_id) => {
                can_view_project(
                    &mut tx,
                    &mut project_views,
                    workspace_id,
                    actor_user_id,
                    project_id,
                )
                .await?
            }
        };
        if !visible {
            continue;
        }
        let display_id = match (project_id, key) {
            (None, _) => Some(format_display_id("WIKI", number)),
            (Some(_), Some(key)) => Some(format_display_id(&key, number)),
            (Some(_), None) => None,
        };
        items.push(Backlink {
            kind: BacklinkKind::Document,
            id,
            title,
            display_id,
        });
    }
    for (id, title, project_id, number, key) in tasks {
        if !can_view_project(
            &mut tx,
            &mut project_views,
            workspace_id,
            actor_user_id,
            project_id,
        )
        .await?
        {
            continue;
        }
        items.push(Backlink {
            kind: BacklinkKind::Task,
            id,
            title,
            display_id: key.map(|key| format_display_id(&key, number)),
        });
    }
    tx.commit().await?;
    // Source orders by the reference row id; the referencing item id is ours.
    items.sort_by_key(|item| item.id);
    Ok(Ok(items))
}

pub async fn list_document_backlinks_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
) -> Result<Result<Vec<Backlink>, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_document_backlinks(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            scope,
            document_id,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if let Err(e) = op.authorize_document_current_scope((workspace_id, document_id), actor_user_id, session_id, scope, ProjectPermission::View).await? {
            return Ok(Err(e));
        }
        let mut items = Vec::new();
        for task in [false, true] {
            let mut after = None;
            loop {
                let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
                let rows = family
                    .query(
                        if task {
                            r#"
SELECT t.id,t.title,t.project_id,t.number,p.key,t.content_json FROM tasks t LEFT JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id AND p.deleted_at IS NULL WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND (?2 IS NULL OR t.id>?2) ORDER BY t.id LIMIT 128
"#
                        } else {
                            r#"
SELECT d.id,d.title,d.project_id,d.number,p.key,d.content_json FROM documents d LEFT JOIN projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id AND p.deleted_at IS NULL WHERE d.workspace_id=?1 AND d.deleted_at IS NULL AND (?2 IS NULL OR d.id>?2) ORDER BY d.id LIMIT 128
"#
                        },
                        &[Cell::uuid(workspace_id), Cell::optional_uuid(after)],
                    )
                    .await?;
                for row in &rows {
                    let id = row.cell(0)?.id()?;
                    after = Some(id);
                    if !task && id == document_id {
                        continue;
                    }
                    let content = row.cell(5)?.value()?;
                    if !crate::collab::derived_body::extract_internal_refs(&content).iter().any(|r| r.kind == crate::collab::derived_body::InternalRefKind::Document && Uuid::parse_str(&r.id).ok() == Some(document_id)) {
                        continue;
                    }
                    let project = row.cell(2)?.optional(Cell::id)?;
                    if !family_can_view_document(&mut op, workspace_id, actor_user_id, id, project).await? {
                        continue;
                    }
                    let key = row.cell(4)?.optional(Cell::string)?;
                    let number = row.cell(3)?.int32()?;
                    items.push(Backlink {
                        kind: if task { BacklinkKind::Task } else { BacklinkKind::Document },
                        id,
                        title: row.cell(1)?.string()?,
                        display_id: match project {
                            None => Some(format_display_id("WIKI", number)),
                            Some(_) => key.map(|key| format_display_id(&key, number)),
                        },
                    });
                }
                if rows.len() < 128 {
                    break;
                }
            }
        }
        items.sort_by_key(|item| item.id);
        Ok(Ok(items))
    }
    .await;
    finish_document_operation(tx, result).await
}

async fn family_can_view_document(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    document: Uuid,
    project: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    Ok(match project {
        None => op
            .document_permission(workspace, actor, document, true)
            .await?
            .at_least(ProjectPermission::View),
        Some(project) => op
            .project_permission_by_id(workspace, actor, project)
            .await?
            .is_some_and(|p| p.at_least(ProjectPermission::View)),
    })
}

async fn family_source_body(
    family: &mut crate::db::backend::FamilyTx,
    workspace: Uuid,
    document: Uuid,
) -> Result<Option<SourceBody>, sqlx::Error> {
    family.require_tenant(workspace)?;
    let rows=family.query("SELECT ds.state,ds.encoding,u.payload FROM document_states ds LEFT JOIN document_collab_updates u ON u.workspace_id=ds.workspace_id AND u.document_id=ds.document_id AND u.seq>ds.snapshot_cutoff_seq AND u.seq<=ds.tail_seq WHERE ds.workspace_id=?1 AND ds.document_id=?2 ORDER BY u.seq", &[Cell::uuid(workspace),Cell::uuid(document)]).await?;
    if let Some(row) = rows.first() {
        if row.cell(1)?.integer()? != i64::from(COLLAB_STATE_ENCODING_V1) {
            return Ok(None);
        }
        return Ok(Some(SourceBody::Collab {
            snapshot: row.cell(0)?.bytes()?,
            tail: rows
                .iter()
                .filter_map(|r| match r.cell(2).and_then(|c| c.optional(Cell::bytes)) {
                    Ok(None) => None,
                    Ok(Some(bytes)) => Some(Ok(bytes)),
                    Err(e) => Some(Err(e)),
                })
                .collect::<Result<Vec<_>, _>>()?,
        }));
    }
    family
        .query(
            "SELECT content_json FROM documents WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(workspace), Cell::uuid(document)],
        )
        .await?
        .first()
        .map(|row| Ok(SourceBody::Json(row.cell(0)?.value()?)))
        .transpose()
}

async fn can_view_project(
    tx: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<Uuid, bool>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Some(visible) = cache.get(&project_id) {
        return Ok(*visible);
    }
    let visible = project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
        .await?
        .is_some_and(|p| p.at_least(ProjectPermission::View));
    cache.insert(project_id, visible);
    Ok(visible)
}

/// Persisted body of one document to copy: collab state when it exists,
/// otherwise the stored JSON (source `latestSourceJson`).
#[derive(Debug, Clone)]
pub enum SourceBody {
    Json(Value),
    Collab {
        snapshot: Vec<u8>,
        tail: Vec<Vec<u8>>,
    },
}

#[derive(Debug, Clone)]
pub struct DuplicateSource {
    pub id: Uuid,
    /// Source parent inside the copied subtree; `None` for the root.
    pub parent_in_copy: Option<Uuid>,
    pub body: SourceBody,
}

/// Prepared body of one copy: projected JSON, derived text and the independent seed.
#[derive(Debug, Clone)]
pub struct DuplicateBody {
    pub source_id: Uuid,
    pub content_json: Value,
    pub text: String,
    pub chosung: String,
    pub seed: Vec<u8>,
}

pub(crate) async fn load_source_body(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<SourceBody>, sqlx::Error> {
    // One statement: the state row and its tail are read from the same snapshot.
    let rows = sqlx::query_as::<_, (Vec<u8>, i16, Option<Vec<u8>>)>(
        r#"
        SELECT ds.state, ds.encoding, u.payload
        FROM fvoci.document_states ds
        LEFT JOIN fvoci.document_collab_updates u
          ON u.workspace_id = ds.workspace_id
         AND u.document_id = ds.document_id
         AND u.seq > ds.snapshot_cutoff_seq
         AND u.seq <= ds.tail_seq
        WHERE ds.workspace_id = $1 AND ds.document_id = $2
        ORDER BY u.seq ASC
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut **tx)
    .await?;
    if let Some((snapshot, encoding, _)) = rows.first() {
        if *encoding != COLLAB_STATE_ENCODING_V1 {
            return Ok(None);
        }
        let snapshot = snapshot.clone();
        let tail = rows
            .into_iter()
            .filter_map(|(_, _, payload)| payload)
            .collect();
        return Ok(Some(SourceBody::Collab { snapshot, tail }));
    }
    let json: Option<(Value,)> = sqlx::query_as(
        "SELECT content_json FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(json.map(|(v,)| SourceBody::Json(v)))
}

/// Live children in tree order (source `repos.documents.listChildren`).
async fn live_children(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<Vec<(Uuid, Option<Uuid>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT id, project_id
        FROM fvoci.documents
        WHERE workspace_id = $1 AND parent_id = $2 AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await
}

async fn can_view_document(
    tx: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<Uuid, bool>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    match project_id {
        None => Ok(
            document_permission(tx, workspace_id, actor_user_id, document_id, true)
                .await?
                .at_least(ProjectPermission::View),
        ),
        Some(project_id) => {
            can_view_project(tx, cache, workspace_id, actor_user_id, project_id).await
        }
    }
}

/// Phase 1 of duplicate: edit access on the root and the persisted bodies of
/// the root and (when asked) every viewable descendant, preorder. The copies
/// reflect the sources as of this read; phase 2 rechecks access and commits.
pub async fn load_duplicate_sources(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    include_children: bool,
) -> Result<Result<Vec<DuplicateSource>, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = scoped_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        scope,
        document_id,
        ProjectPermission::Edit,
        // Phase 1 only reads; phase 2 rechecks under the lock.
        false,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let Some(root_body) = load_source_body(&mut tx, workspace_id, document_id).await? else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let mut sources = vec![DuplicateSource {
        id: document_id,
        parent_in_copy: None,
        body: root_body,
    }];
    if include_children {
        let mut cache = HashMap::new();
        // Depth-first preorder, children in tree order (source `copyDescendants`).
        let mut stack = vec![document_id];
        let mut order = Vec::new();
        while let Some(parent) = stack.pop() {
            let kids = live_children(&mut tx, workspace_id, parent).await?;
            let mut visible = Vec::new();
            for (kid, project_id) in kids {
                if can_view_document(
                    &mut tx,
                    &mut cache,
                    workspace_id,
                    actor_user_id,
                    kid,
                    project_id,
                )
                .await?
                {
                    visible.push(kid);
                }
            }
            for kid in visible.iter().rev() {
                stack.push(*kid);
            }
            for kid in visible {
                order.push((kid, parent));
            }
        }
        // `order` groups siblings per parent; rebuild a true preorder from the parent links.
        let mut by_parent: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (kid, parent) in order {
            by_parent.entry(parent).or_default().push(kid);
        }
        let mut walk = vec![document_id];
        while let Some(parent) = walk.pop() {
            if let Some(kids) = by_parent.get(&parent) {
                for kid in kids {
                    let Some(body) = load_source_body(&mut tx, workspace_id, *kid).await? else {
                        continue;
                    };
                    sources.push(DuplicateSource {
                        id: *kid,
                        parent_in_copy: Some(parent),
                        body,
                    });
                }
                for kid in kids.iter().rev() {
                    walk.push(*kid);
                }
            }
        }
    }
    tx.commit().await?;
    Ok(Ok(sources))
}

pub async fn load_duplicate_sources_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    include_children: bool,
) -> Result<Result<Vec<DuplicateSource>, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return load_duplicate_sources(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            scope,
            document_id,
            include_children,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if let Err(e) = op.authorize_document_current_scope((workspace_id, document_id), actor_user_id, session_id, scope, ProjectPermission::Edit).await? {
            return Ok(Err(e));
        }
        let mut sources = Vec::new();
        let mut stack = vec![(document_id, None)];
        while let Some((id, parent)) = stack.pop() {
            let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
            let Some(body) = family_source_body(family, workspace_id, id).await? else {
                if parent.is_none() {
                    return Ok(Err(DocumentDbError::NotFound));
                }
                continue;
            };
            sources.push(DuplicateSource { id, parent_in_copy: parent, body });
            if !include_children {
                break;
            }
            let rows = family
                .query(
                    r#"
SELECT id,project_id FROM documents WHERE workspace_id=?1 AND parent_id=?2 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY,id
"#,
                    &[Cell::uuid(workspace_id), Cell::uuid(id)],
                )
                .await?;
            for row in rows.iter().rev() {
                let kid = row.cell(0)?.id()?;
                if family_can_view_document(&mut op, workspace_id, actor_user_id, kid, row.cell(1)?.optional(Cell::id)?).await? {
                    stack.push((kid, Some(id)));
                }
            }
        }
        Ok(Ok(sources))
    }
    .await;
    finish_document_operation(tx, result).await
}

type CopyRow = (
    Option<Uuid>,
    Option<Uuid>,
    String,
    Option<String>,
    String,
    Option<DateTime<Utc>>,
);

async fn fetch_copy_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<CopyRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT parent_id, project_id, title, icon, sort_key, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

/// Source `doc.duplicate.suffix` (ko): "{{title}} (복사)", or the plain title when too long.
pub fn duplicate_title(source_title: &str) -> String {
    let labeled = format!("{source_title} (복사)");
    if crate::db::documents::title_is_valid(&labeled) {
        labeled
    } else {
        source_title.to_string()
    }
}

/// Phase 2 of duplicate (source `duplicateDocument`): one transaction under the
/// tree lock rechecks the actor, the root's edit access and every copied
/// node's view access, then inserts each copy with its body, independent collab
/// seed and tags, next to the source (root) or under its copied parent.
pub async fn commit_duplicate(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    title_override: Option<&str>,
    sources: &[DuplicateSource],
    bodies: &[DuplicateBody],
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let Some(root) = sources.first() else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    let body_of: HashMap<Uuid, &DuplicateBody> = bodies.iter().map(|b| (b.source_id, b)).collect();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    if let Err(err) = scoped_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        scope,
        root.id,
        ProjectPermission::Edit,
        true,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    // Source `createDocumentIn` for a wiki copy checks the workspace base role
    // (group grants on the source document do not allow creating documents).
    if scope.project_id().is_none() {
        let role =
            crate::db::workspace::membership_role_for_update(&mut tx, workspace_id, actor_user_id)
                .await?;
        if !crate::db::documents::wiki_can_edit(role) {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::Forbidden));
        }
    }

    let mut cache = HashMap::new();
    // source id -> (copy id, copy path)
    let mut copied: HashMap<Uuid, (Uuid, String)> = HashMap::new();
    let mut root_copy = None;
    for source in sources {
        let Some(body) = body_of.get(&source.id) else {
            continue;
        };
        let Some((parent_id, project_id, title, icon, sort_key, deleted_at)) =
            fetch_copy_row(&mut tx, workspace_id, source.id).await?
        else {
            if source.parent_in_copy.is_none() {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            }
            continue;
        };
        if deleted_at.is_some() || project_id != scope.project_id() {
            if source.parent_in_copy.is_none() {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::NotFound));
            }
            continue;
        }
        let (copy_parent, copy_title, copy_sort_key, parent_path) = match source.parent_in_copy {
            None => {
                // Source `assertCreateAffiliation`: a project document needs a
                // parent, so the project root cannot be duplicated.
                if scope.project_id().is_some() && parent_id.is_none() {
                    tx.rollback().await?;
                    return Ok(Err(DocumentDbError::AffiliationMismatch));
                }
                let next: Option<(String,)> = sqlx::query_as(
                    r#"
                    SELECT sort_key
                    FROM fvoci.documents
                    WHERE workspace_id = $1
                      AND parent_id IS NOT DISTINCT FROM $2
                      AND deleted_at IS NULL
                      AND sort_key COLLATE "C" > $3 COLLATE "C"
                    ORDER BY sort_key COLLATE "C"
                    LIMIT 1
                    "#,
                )
                .bind(workspace_id)
                .bind(parent_id)
                .bind(&sort_key)
                .fetch_optional(&mut *tx)
                .await?;
                let key = match between(Some(&sort_key), next.as_ref().map(|(k,)| k.as_str())) {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::error!("{err}");
                        tx.rollback().await?;
                        return Ok(Err(DocumentDbError::InvalidSortKey));
                    }
                };
                let parent_path = match parent_id {
                    None => None,
                    Some(parent_id) => {
                        let row: Option<(String,)> = sqlx::query_as(
                            "SELECT path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
                        )
                        .bind(workspace_id)
                        .bind(parent_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                        let Some((path,)) = row else {
                            tx.rollback().await?;
                            return Ok(Err(DocumentDbError::NotFound));
                        };
                        Some(path)
                    }
                };
                let title = match title_override {
                    Some(t) => t.to_string(),
                    None => duplicate_title(&title),
                };
                (parent_id, title, key, parent_path)
            }
            Some(source_parent) => {
                let Some((copy_parent, copy_parent_path)) = copied.get(&source_parent).cloned()
                else {
                    // Its copied parent was skipped: skip the whole branch.
                    continue;
                };
                if !can_view_document(
                    &mut tx,
                    &mut cache,
                    workspace_id,
                    actor_user_id,
                    source.id,
                    project_id,
                )
                .await?
                {
                    continue;
                }
                let last: Option<(String,)> = sqlx::query_as(
                    r#"
                    SELECT sort_key
                    FROM fvoci.documents
                    WHERE workspace_id = $1 AND parent_id = $2 AND deleted_at IS NULL
                    ORDER BY sort_key COLLATE "C" DESC
                    LIMIT 1
                    "#,
                )
                .bind(workspace_id)
                .bind(copy_parent)
                .fetch_optional(&mut *tx)
                .await?;
                let key = match between(last.as_ref().map(|(k,)| k.as_str()), None) {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::error!("{err}");
                        tx.rollback().await?;
                        return Ok(Err(DocumentDbError::InvalidSortKey));
                    }
                };
                (Some(copy_parent), title, key, Some(copy_parent_path))
            }
        };

        let copy_id = Uuid::now_v7();
        let path = match &parent_path {
            Some(parent_path) => format!("{parent_path}.{}", to_path_label(copy_id)),
            None => to_path_label(copy_id),
        };
        let number: (i32,) = match project_id {
            None => {
                sqlx::query_as(
                    r#"
                    UPDATE fvoci.workspaces
                    SET next_document_number = next_document_number + 1, updated_at = now()
                    WHERE id = $1 AND deleted_at IS NULL
                    RETURNING next_document_number
                    "#,
                )
                .bind(workspace_id)
                .fetch_one(&mut *tx)
                .await?
            }
            Some(project_id) => {
                sqlx::query_as(
                    r#"
                    UPDATE fvoci.projects
                    SET next_number = next_number + 1, updated_at = now()
                    WHERE workspace_id = $1 AND id = $2
                    RETURNING next_number - 1
                    "#,
                )
                .bind(workspace_id)
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        sqlx::query(
            r#"
            INSERT INTO fvoci.documents (
                id, workspace_id, title, icon, path, parent_id, sort_key, project_id,
                number, status, schema_version, content_json, text, chosung, created_by, kind
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8,
                $9, 'draft', $10, $11, $12, $13, $14, 'doc'
            )
            "#,
        )
        .bind(copy_id)
        .bind(workspace_id)
        .bind(&copy_title)
        .bind(&icon)
        .bind(&path)
        .bind(copy_parent)
        .bind(&copy_sort_key)
        .bind(project_id)
        .bind(number.0)
        .bind(DOCUMENT_SCHEMA_VERSION)
        .bind(&body.content_json)
        .bind(&body.text)
        .bind(&body.chosung)
        .bind(actor_user_id)
        .execute(&mut *tx)
        .await?;
        // Independent collab state seeded from the copied body (source `seedIndependentState`).
        sqlx::query(
            r#"
            INSERT INTO fvoci.document_states (
                workspace_id, document_id, state, encoding,
                writer_generation, snapshot_cutoff_seq, tail_seq
            ) VALUES ($1, $2, $3, $4, 0, 0, 0)
            "#,
        )
        .bind(workspace_id)
        .bind(copy_id)
        .bind(&body.seed)
        .bind(COLLAB_STATE_ENCODING_V1)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO fvoci.document_tag_assignments (workspace_id, document_id, tag_id)
            SELECT workspace_id, $3, tag_id
            FROM fvoci.document_tag_assignments
            WHERE workspace_id = $1 AND document_id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(source.id)
        .bind(copy_id)
        .execute(&mut *tx)
        .await?;
        record_document_event_and_audit(
            &mut tx,
            workspace_id,
            actor_user_id,
            "document.created",
            copy_id,
            json!({
                "documentId": copy_id.to_string(),
                "parentId": copy_parent.map(|id| id.to_string()),
                "title": copy_title,
                "projectId": project_id.map(|id| id.to_string()),
                "duplicatedFrom": source.id.to_string(),
            }),
            client_ip,
        )
        .await?;
        copied.insert(source.id, (copy_id, path));
        if source.parent_in_copy.is_none() {
            root_copy = Some(copy_id);
        }
    }

    let Some(root_copy) = root_copy else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let meta = match scoped_meta(&mut tx, workspace_id, scope, root_copy).await? {
        Some(mut meta) => {
            if meta.display_id.is_none() {
                meta.display_id = Some(format_display_id("WIKI", meta.number));
            }
            meta
        }
        None => {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        }
    };
    tx.commit().await?;
    Ok(Ok(meta))
}

pub async fn commit_duplicate_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: DocumentScope,
    title_override: Option<&str>,
    sources: &[DuplicateSource],
    bodies: &[DuplicateBody],
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return commit_duplicate(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            scope,
            title_override,
            sources,
            bodies,
            client_ip,
        )
        .await;
    }
    let Some(root) = sources.first() else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    let body_of: HashMap<_, _> = bodies.iter().map(|b| (b.source_id, b)).collect();
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        op.lock_membership_users(&[actor_user_id]).await?;
        if !op.recheck_session(actor_user_id, session_id).await? {
            return Ok(Err(DocumentDbError::Forbidden));
        }
        op.lock_tree(workspace_id).await?;
        if let Err(e) = op.authorize_document_current_scope((workspace_id, root.id), actor_user_id, session_id, scope, ProjectPermission::Edit).await? {
            return Ok(Err(e));
        }
        if scope.project_id().is_none() && !crate::db::documents::wiki_can_edit(op.membership_role(workspace_id, actor_user_id, true).await?) {
            return Ok(Err(DocumentDbError::Forbidden));
        }
        let mut copied: HashMap<Uuid, (Uuid, String)> = HashMap::new();
        let mut root_copy = None;
        for source in sources {
            let Some(body) = body_of.get(&source.id) else {
                continue;
            };
            let Some(current) = op.document_row(workspace_id, source.id).await? else {
                if source.parent_in_copy.is_none() {
                    return Ok(Err(DocumentDbError::NotFound));
                }
                continue;
            };
            if current.8 != scope.project_id() {
                if source.parent_in_copy.is_none() {
                    return Ok(Err(DocumentDbError::NotFound));
                }
                continue;
            }
            let (parent, title, sort, parent_path) = match source.parent_in_copy {
                None => {
                    if scope.project_id().is_some() && current.6.is_none() {
                        return Ok(Err(DocumentDbError::AffiliationMismatch));
                    }
                    let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
                    let next = family
                        .query(
                            r#"
SELECT sort_key FROM documents WHERE workspace_id=?1 AND parent_id IS ?2 AND deleted_at IS NULL AND sort_key COLLATE BINARY>?3 ORDER BY sort_key COLLATE BINARY LIMIT 1
"#,
                            &[Cell::uuid(workspace_id), Cell::optional_uuid(current.6), Cell::text(&current.7)],
                        )
                        .await?;
                    let next = next.first().map(|r| r.cell(0)?.string()).transpose()?;
                    let Ok(sort) = between(Some(&current.7), next.as_deref()) else {
                        return Ok(Err(DocumentDbError::InvalidSortKey));
                    };
                    let path = match current.6 {
                        None => None,
                        Some(parent) => {
                            let Some((_, path, _)) = op.wiki_parent(workspace_id, parent).await? else {
                                return Ok(Err(DocumentDbError::NotFound));
                            };
                            Some(path)
                        }
                    };
                    (current.6, title_override.map(str::to_string).unwrap_or_else(|| duplicate_title(&current.2)), sort, path)
                }
                Some(source_parent) => {
                    let Some((parent, path)) = copied.get(&source_parent).cloned() else {
                        continue;
                    };
                    if !family_can_view_document(&mut op, workspace_id, actor_user_id, source.id, current.8).await? {
                        continue;
                    }
                    let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
                    let rows = family
                        .query(
                            r#"
SELECT sort_key FROM documents WHERE workspace_id=?1 AND parent_id=?2 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY DESC LIMIT 1
"#,
                            &[Cell::uuid(workspace_id), Cell::uuid(parent)],
                        )
                        .await?;
                    let last = rows.first().map(|r| r.cell(0)?.string()).transpose()?;
                    let Ok(sort) = between(last.as_deref(), None) else {
                        return Ok(Err(DocumentDbError::InvalidSortKey));
                    };
                    (Some(parent), current.2, sort, Some(path))
                }
            };
            let id = Uuid::now_v7();
            let path = parent_path.map_or_else(|| to_path_label(id), |path| format!("{path}.{}", to_path_label(id)));
            let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
            let number = if let Some(project) = current.8 {
                family
                    .query(
                        r#"
UPDATE projects SET next_number=next_number+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2 AND next_number<2147483647 RETURNING next_number-1
"#,
                        &[Cell::uuid(workspace_id), Cell::uuid(project)],
                    )
                    .await?
            } else {
                family
                    .query(
                        r#"
UPDATE workspaces SET next_document_number=next_document_number+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND deleted_at IS NULL AND next_document_number<2147483647 RETURNING next_document_number
"#,
                        &[Cell::uuid(workspace_id)],
                    )
                    .await?
            };
            let number = number.first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.int32()?;
            family
                .execute(
                    r#"
INSERT INTO documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,text,chosung,created_by,kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'draft',?10,?11,?12,?13,?14,'doc')
"#,
                    &[Cell::uuid(id), Cell::uuid(workspace_id), Cell::text(&title), Cell::optional_text(current.4.as_deref()), Cell::text(&path), Cell::optional_uuid(parent), Cell::text(sort), Cell::optional_uuid(current.8), Cell::Integer(i64::from(number)), Cell::Integer(i64::from(DOCUMENT_SCHEMA_VERSION)), Cell::json(&body.content_json)?, Cell::text(&body.text), Cell::text(&body.chosung), Cell::uuid(actor_user_id)],
                )
                .await?;
            family
                .execute(
                    r#"
INSERT INTO document_states(workspace_id,document_id,state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq) VALUES(?1,?2,?3,?4,0,0,0)
"#,
                    &[Cell::uuid(workspace_id), Cell::uuid(id), Cell::Blob(body.seed.clone()), Cell::Integer(i64::from(COLLAB_STATE_ENCODING_V1))],
                )
                .await?;
            family
                .execute(
                    r#"
INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) SELECT workspace_id,?3,tag_id FROM document_tag_assignments WHERE workspace_id=?1 AND document_id=?2
"#,
                    &[Cell::uuid(workspace_id), Cell::uuid(source.id), Cell::uuid(id)],
                )
                .await?;
            op.record_document_event_and_audit(workspace_id, actor_user_id, "document.created", id, json!({"documentId":id.to_string(),"parentId":parent.map(|id|id.to_string()),"title":title,"projectId":current.8.map(|id|id.to_string()),"duplicatedFrom":source.id.to_string()}), client_ip).await?;
            copied.insert(source.id, (id, path));
            if source.parent_in_copy.is_none() {
                root_copy = Some(id);
            }
        }
        let Some(root_copy) = root_copy else {
            return Ok(Err(DocumentDbError::NotFound));
        };
        let result = op.authorize_document_current_scope((workspace_id, root_copy), actor_user_id, session_id, scope, ProjectPermission::View).await?;
        Ok(result.map(|mut meta| {
            if meta.display_id.is_none() {
                meta.display_id = Some(format_display_id("WIKI", meta.number));
            }
            meta
        }))
    }
    .await;
    finish_document_operation(tx, result).await
}

/// Source `workspaceIdForDocument`: the actor's workspace holding the document
/// (trashed included; the per-route checks decide visibility) and its project.
pub async fn locate_document(
    pool: &PgPool,
    actor_user_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(Uuid, Option<Uuid>)>, sqlx::Error> {
    let mut workspaces: Vec<Uuid> =
        crate::db::workspace::list_workspaces_for_user(pool, actor_user_id)
            .await?
            .into_iter()
            .map(|w| w.id)
            .collect();
    workspaces.sort();
    for workspace_id in workspaces {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let row: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((project_id,)) = row {
            return Ok(Some((workspace_id, project_id)));
        }
    }
    Ok(None)
}

pub async fn locate_document_backend(
    backend: &Backend,
    actor_user_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(Uuid, Option<Uuid>)>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return locate_document(pool, actor_user_id, document_id).await;
    }
    let mut workspaces =
        crate::db::workspace::list_workspaces_for_user_backend(backend, actor_user_id).await?;
    workspaces.sort_by_key(|workspace| workspace.id);
    for workspace in workspaces {
        let mut tx = backend.begin_read().await?;
        let result = async {
            let mut op = tx.operation();
            op.set_tenant(workspace.id).await?;
            let OperationTx::SqliteFamily(family) = op else {
                unreachable!()
            };
            family
                .query(
                    "SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace.id), Cell::uuid(document_id)],
                )
                .await?
                .first()
                .map(|r| r.cell(0)?.optional(Cell::id))
                .transpose()
        }
        .await;
        tx.rollback().await?;
        if let Some(project) = result? {
            return Ok(Some((workspace.id, project)));
        }
    }
    Ok(None)
}

#[derive(Debug, thiserror::Error)]
#[error("document operation refused: {0:?}")]
struct DocumentOperationRefusal(DocumentDbError);

pub(crate) async fn finish_document_operation<T>(
    tx: crate::db::backend::DbTx,
    result: Result<Result<T, DocumentDbError>, sqlx::Error>,
) -> Result<Result<T, DocumentDbError>, sqlx::Error> {
    match result {
        Ok(Ok(value)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(value))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(DocumentOperationRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonpath_only_embeds_canonical_uuid() {
        let id = Uuid::parse_str("0190c6b8-8f3e-7a1b-9c2d-3e4f5a6b7c8d").unwrap();
        let path = internal_ref_jsonpath("document", id);
        assert!(path.contains(r#"like_regex "^0190c6b8-8f3e-7a1b-9c2d-3e4f5a6b7c8d$" flag "i""#));
        assert!(path.contains(r#"@.attrs.entity == "document""#));
    }

    #[test]
    fn duplicate_title_falls_back_when_suffix_overflows() {
        assert_eq!(duplicate_title("회의록"), "회의록 (복사)");
        let long = "가".repeat(300);
        assert_eq!(duplicate_title(&long), long);
    }
}

/// Explicit SDK control; this fixture is selected separately from CI cohorts.
#[cfg(all(test, feature = "db-tests"))]
mod document_port_sdk_tests {
    use super::*;
    use crate::db::backend::{DbTx, RemoteDatabase};
    use futures_util::FutureExt;

    #[tokio::test]
    #[ignore = "explicit pinned official sqld fixture via FVOCI_TEST_SQLD"]
    async fn official_sqld_document_duplicate_backlinks_tags_and_revocation() {
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("explicit pinned official sqld required"),
        );
        let root = std::env::temp_dir().join(format!("fvoci-document-sdk-{}", Uuid::now_v7()));
        let driver = crate::db::libsql_finish_fixture::LibsqlFinishFixture::start(&sqld, &root)
            .await
            .unwrap();
        let db = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            std::num::NonZeroU32::new(2).unwrap(),
        )));
        let body = std::panic::AssertUnwindSafe(async {
        crate::db::migrate::initialize_family_backend_for_test(&db).await.unwrap();
        let workspace = Uuid::now_v7();
        let actor = Uuid::now_v7();
        let credential = Uuid::now_v7();
        let mut tx = db.begin_write().await.unwrap();
        let DbTx::SqliteFamily(family) = &mut tx else { unreachable!() };
        family.replace_system_context(true);
        family.execute("INSERT INTO users(id,email,given_name) VALUES(?1,'document-sdk@example.invalid','Owner')", &[Cell::uuid(actor)]).await.unwrap();
        family.execute("INSERT INTO workspaces(id,slug,name) VALUES(?1,'document-sdk','Documents')", &[Cell::uuid(workspace)]).await.unwrap();
        family.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')", &[Cell::uuid(workspace), Cell::uuid(actor)]).await.unwrap();
        family
            .execute(
                r#"
INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'document-sdk-synthetic',(unixepoch()+3600)*1000000)
"#,
                &[Cell::uuid(credential), Cell::uuid(actor)],
            )
            .await
            .unwrap();
        tx.commit_with_cleanup().await.unwrap();
        let document = crate::db::documents::create_wiki_document_command_backend(&db, workspace, actor, credential, Uuid::now_v7(), crate::db::documents::CreateDocumentInput { parent_id: None, title: "SDK original", icon: None }, None).await.unwrap().unwrap();
        let tag_actor = crate::db::collections::Actor { user_id: actor, credential_id: credential, client_ip: None };
        let tag = crate::db::document_tags::create_tag_backend(&db, workspace, &tag_actor, "SDK tag", "red").await.unwrap().unwrap();
        crate::db::document_tags::assign_tag_backend(&db, workspace, &tag_actor, document.id, crate::db::document_tags::Affiliation::Wiki, tag.id).await.unwrap().unwrap();
        let sources = load_duplicate_sources_backend(&db, workspace, actor, credential, DocumentScope::Wiki, document.id, false).await.unwrap().unwrap();
        let json = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"mention","attrs":{"entity":"document","id":document.id.to_string()}}]}]});
        let engine = crate::collab::CollabConfig::from_env().expect("native seed helper required");
        let seed = crate::collab::seed::SeedEngine::new(engine.engine_bin, engine.limits).tiptap_to_yjs_update(&json).await.unwrap();
        let bodies = vec![DuplicateBody { source_id: document.id, content_json: json.clone(), text: String::new(), chosung: String::new(), seed }];
        let copy = commit_duplicate_backend(&db, workspace, actor, credential, DocumentScope::Wiki, None, &sources, &bodies, None).await.unwrap().unwrap();
        let fresh = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(driver.database().await.unwrap(), std::num::NonZeroU32::new(1).unwrap())));
        let read = authorize_document_backend(&fresh, workspace, actor, credential, DocumentScope::Wiki, copy.id, ProjectPermission::View).await.unwrap().unwrap();
        assert_eq!(read.content_json, json);
        assert_ne!(copy.id, document.id);
        let links = list_document_backlinks_backend(&fresh, workspace, actor, credential, DocumentScope::Wiki, document.id).await.unwrap().unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].id, copy.id);
        let tags = crate::db::document_tags::list_document_tags_backend(&fresh, workspace, &tag_actor, copy.id, crate::db::document_tags::Affiliation::Wiki).await.unwrap().unwrap();
        assert_eq!(tags[0].id, tag.id);
        assert_eq!(crate::db::document_tags::update_tag_backend(&fresh, workspace, &tag_actor, tag.id, Some("SDK renamed"), None).await.unwrap().unwrap().name, "SDK renamed");
        crate::db::document_tags::unassign_tag_backend(&fresh, workspace, &tag_actor, copy.id, crate::db::document_tags::Affiliation::Wiki, tag.id).await.unwrap().unwrap();
        crate::db::document_tags::delete_tag_backend(&fresh, workspace, &tag_actor, tag.id).await.unwrap().unwrap();
        assert!(matches!(load_duplicate_sources_backend(&fresh, Uuid::now_v7(), actor, credential, DocumentScope::Wiki, document.id, false).await.unwrap(), Err(DocumentDbError::NotFound)));
        let mut tx = db.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(workspace).await.unwrap();
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        family.execute("UPDATE sessions SET revoked_at=unixepoch()*1000000 WHERE id=?1", &[Cell::uuid(credential)]).await.unwrap();
        tx.commit_with_cleanup().await.unwrap();
        assert!(matches!(commit_duplicate_backend(&fresh, workspace, actor, credential, DocumentScope::Wiki, None, &sources, &bodies, None).await.unwrap(), Err(DocumentDbError::Forbidden)));
        fresh.close().await.unwrap();
    })
    .catch_unwind()
    .await;
        let close = db.close().await;
        let finish = driver.finish().await;
        if let Err(primary) = body {
            eprintln!("document SDK cleanup close={close:?} fixture={finish:?}");
            std::panic::resume_unwind(primary);
        }
        close.unwrap();
        assert!(finish.unwrap().sqld_reaped);
        std::fs::remove_dir_all(root).unwrap();
    }
}
