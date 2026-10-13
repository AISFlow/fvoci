//! Workspace document/task templates (source `packages/core/src/template.ts`).
#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::collections::{begin_member, scope_access, Actor, CollectionDbError};
use crate::db::context::set_tenant;
use crate::db::documents::{
    create_wiki_document, create_wiki_document_operation, CreateDocumentInput, DocumentDbError,
    DocumentMeta,
};
use crate::db::project_documents::{create_project_document, create_project_document_backend};
use crate::db::project_views::begin_member_family;
use crate::db::projects::{lock_project, ProjectDbError};
use crate::db::tasks::{create_task, create_task_backend, CreateTaskInput};
use crate::display_id::format_display_id;
use crate::projects::{workspace_base_permission, ProjectPermission};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    Document,
    Task,
}

impl TemplateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Task => "task",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "document" => Some(Self::Document),
            "task" => Some(Self::Task),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateDbError {
    NotFound,
    Forbidden,
    InvalidInput,
    ProjectArchived,
}

pub type TemplateResult<T> = Result<Result<T, TemplateDbError>, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct TemplateRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub kind: TemplateKind,
    pub title: String,
    pub payload: Value,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn from_collection(err: CollectionDbError) -> TemplateDbError {
    match err {
        CollectionDbError::ProjectArchived | CollectionDbError::TaskArchived => {
            TemplateDbError::ProjectArchived
        }
        CollectionDbError::Forbidden => TemplateDbError::Forbidden,
        _ => TemplateDbError::NotFound,
    }
}

// PostgreSQL text and jsonb refuse NUL; the SQLite family would store it.
// Refuse it here so every backend answers 400 before any write.
pub fn title_is_valid(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && !title.contains('\0') && trimmed.chars().count() <= 200
}

fn json_contains_nul(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(items) => items.iter().any(json_contains_nul),
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| key.contains('\0') || json_contains_nul(item)),
        _ => false,
    }
}

/// Bounded payload: JSON object; optional `title` must be a string when present.
pub fn normalize_payload(payload: Value) -> Result<Value, TemplateDbError> {
    let Some(obj) = payload.as_object() else {
        return Err(TemplateDbError::InvalidInput);
    };
    if json_contains_nul(&payload) {
        return Err(TemplateDbError::InvalidInput);
    }
    if let Some(title) = obj.get("title") {
        if !title.is_string() {
            return Err(TemplateDbError::InvalidInput);
        }
    }
    Ok(payload)
}

fn row_from_tuple(
    id: Uuid,
    workspace_id: Uuid,
    kind: String,
    title: String,
    payload: Value,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Option<TemplateRow> {
    let kind = TemplateKind::parse(&kind)?;
    Some(TemplateRow {
        id,
        workspace_id,
        kind,
        title,
        payload,
        created_by,
        created_at,
        updated_at,
    })
}

fn family_template_row(row: &FamilyRow) -> Result<TemplateRow, sqlx::Error> {
    row_from_tuple(
        row.cell(0)?.id()?,
        row.cell(1)?.id()?,
        row.cell(2)?.string()?,
        row.cell(3)?.string()?,
        row.cell(4)?.value()?,
        row.cell(5)?.id()?,
        row.cell(6)?.datetime()?,
        row.cell(7)?.datetime()?,
    )
    .ok_or_else(|| sqlx::Error::Protocol("template kind outside document|task".into()))
}

/// The two kinds a caller may see, as `kind IN (?a,?b)` arguments; a missing
/// kind binds NULL and matches nothing.
fn family_kind_cells(kinds: &[TemplateKind]) -> [Cell; 2] {
    let mut cells = [Cell::Null, Cell::Null];
    for (cell, kind) in cells.iter_mut().zip(kinds) {
        *cell = Cell::text(kind.as_str());
    }
    cells
}

#[derive(Debug, thiserror::Error)]
#[error("template operation refused: {0:?}")]
struct TemplateRefusal(TemplateDbError);

/// Release of a family transaction that wrote nothing: an unknown cleanup
/// withholds the observation and keeps its first cause.
fn template_read_after_rollback<T>(
    result: TemplateResult<T>,
    cleanup: Result<(), sqlx::Error>,
) -> TemplateResult<T> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(TemplateRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
}

/// Finish of a family template write: only `Ok(Ok)` commits, and an uncertain
/// finish is not an ACK.
async fn finish_template_write<T>(tx: DbTx, result: TemplateResult<T>) -> TemplateResult<T> {
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
                    Some(Box::new(TemplateRefusal(refusal))),
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

pub async fn list_templates(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    kinds: &[TemplateKind],
) -> TemplateResult<Vec<TemplateRow>> {
    let mut tx = pool.begin().await?;
    let role = match begin_member(&mut tx, workspace_id, actor, false).await? {
        Ok(role) => role,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(from_collection(err)));
        }
    };
    if !workspace_base_permission(role).at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(TemplateDbError::NotFound));
    }
    if kinds.is_empty() {
        tx.commit().await?;
        return Ok(Ok(vec![]));
    }
    let kind_filter: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    let rows: Vec<(
        Uuid,
        Uuid,
        String,
        String,
        Value,
        Uuid,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, workspace_id, kind, title, payload, created_by, created_at, updated_at \
         FROM fvoci.templates WHERE workspace_id = $1 AND kind = ANY($2::text[]) \
         ORDER BY created_at DESC, id DESC",
    )
    .bind(workspace_id)
    .bind(&kind_filter)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(
            row_from_tuple(r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7)
                .expect("templates.kind_check enforces document|task"),
        );
    }
    Ok(Ok(out))
}

/// [`list_templates`] for every backend: Edit base permission, then the
/// caller's kinds newest first.
pub async fn list_templates_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    kinds: &[TemplateKind],
) -> TemplateResult<Vec<TemplateRow>> {
    if let Backend::Postgres(pool) = backend {
        return list_templates(pool, workspace_id, actor, kinds).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        let Some(role) = begin_member_family(&mut op, workspace_id, actor, false).await? else {
            return Ok(Err(TemplateDbError::NotFound));
        };
        if !workspace_base_permission(role).at_least(ProjectPermission::Edit) {
            return Ok(Err(TemplateDbError::NotFound));
        }
        if kinds.is_empty() {
            return Ok(Ok(vec![]));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let [first, second] = family_kind_cells(kinds);
        let rows = family
            .query(
                "SELECT id,workspace_id,kind,title,payload,created_by,created_at,updated_at
                 FROM templates WHERE workspace_id=?1 AND kind IN (?2,?3)
                 ORDER BY created_at DESC,id DESC",
                &[Cell::uuid(workspace_id), first, second],
            )
            .await?;
        Ok(Ok(rows
            .iter()
            .map(family_template_row)
            .collect::<Result<Vec<_>, sqlx::Error>>()?))
    }
    .await;
    let cleanup = tx.rollback().await;
    template_read_after_rollback(result, cleanup)
}

pub async fn create_template(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    kind: TemplateKind,
    title: &str,
    payload: Value,
) -> TemplateResult<TemplateRow> {
    if !title_is_valid(title) {
        return Ok(Err(TemplateDbError::InvalidInput));
    }
    let payload = match normalize_payload(payload) {
        Ok(v) => v,
        Err(err) => return Ok(Err(err)),
    };
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    let role = match begin_member(&mut tx, workspace_id, actor, true).await? {
        Ok(role) => role,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(from_collection(err)));
        }
    };
    if !workspace_base_permission(role).at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(TemplateDbError::NotFound));
    }
    let title = title.trim();
    let inserted: Option<(
        Uuid,
        Uuid,
        String,
        String,
        Value,
        Uuid,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "INSERT INTO fvoci.templates (id, workspace_id, kind, title, payload, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, workspace_id, kind, title, payload, created_by, created_at, updated_at",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(kind.as_str())
    .bind(title)
    .bind(payload)
    .bind(actor.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let row = inserted.and_then(|r| row_from_tuple(r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7));
    Ok(row.ok_or(TemplateDbError::NotFound))
}

/// [`create_template`] for every backend; input is refused before any write.
pub async fn create_template_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    kind: TemplateKind,
    title: &str,
    payload: Value,
) -> TemplateResult<TemplateRow> {
    if let Backend::Postgres(pool) = backend {
        return create_template(pool, workspace_id, actor, kind, title, payload).await;
    }
    if !title_is_valid(title) {
        return Ok(Err(TemplateDbError::InvalidInput));
    }
    let payload = match normalize_payload(payload) {
        Ok(v) => v,
        Err(err) => return Ok(Err(err)),
    };
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        let Some(role) = begin_member_family(&mut op, workspace_id, actor, true).await? else {
            return Ok(Err(TemplateDbError::NotFound));
        };
        if !workspace_base_permission(role).at_least(ProjectPermission::Edit) {
            return Ok(Err(TemplateDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let rows = family
            .query(
                "INSERT INTO templates(id,workspace_id,kind,title,payload,created_by) VALUES(?1,?2,?3,?4,?5,?6)
                 RETURNING id,workspace_id,kind,title,payload,created_by,created_at,updated_at",
                &[
                    Cell::uuid(Uuid::now_v7()),
                    Cell::uuid(workspace_id),
                    Cell::text(kind.as_str()),
                    Cell::text(title.trim()),
                    Cell::json(&payload)?,
                    Cell::uuid(actor.user_id),
                ],
            )
            .await?;
        Ok(Ok(family_template_row(
            rows.first().ok_or(sqlx::Error::RowNotFound)?,
        )?))
    }
    .await;
    finish_template_write(tx, result).await
}

pub struct ApplyTemplateInput {
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
}

pub struct AppliedTemplate {
    pub kind: TemplateKind,
    pub id: Uuid,
    pub display_id: String,
}

async fn find_template(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    template_id: Uuid,
    kinds: &[TemplateKind],
) -> Result<Option<TemplateRow>, sqlx::Error> {
    if kinds.is_empty() {
        return Ok(None);
    }
    let kind_filter: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    let row: Option<(
        Uuid,
        Uuid,
        String,
        String,
        Value,
        Uuid,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, workspace_id, kind, title, payload, created_by, created_at, updated_at \
         FROM fvoci.templates WHERE workspace_id = $1 AND id = $2 AND kind = ANY($3::text[])",
    )
    .bind(workspace_id)
    .bind(template_id)
    .bind(&kind_filter)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|r| row_from_tuple(r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7)))
}

fn resolved_title(template: &TemplateRow) -> String {
    if let Some(s) = template.payload.get("title").and_then(Value::as_str) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    template.title.clone()
}

pub async fn apply_template(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    session_id: Uuid,
    template_id: Uuid,
    input: ApplyTemplateInput,
    kinds: &[TemplateKind],
    channel: &str,
    client_ip: Option<&str>,
) -> TemplateResult<AppliedTemplate> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let role = match begin_member(&mut tx, workspace_id, actor, true).await? {
        Ok(role) => role,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(from_collection(err)));
        }
    };
    let access = scope_access(
        &mut tx,
        workspace_id,
        actor.user_id,
        role,
        input.project_id,
        true,
    )
    .await?;
    let Some(access) = access else {
        tx.rollback().await?;
        return Ok(Err(TemplateDbError::NotFound));
    };
    if access.archived {
        tx.rollback().await?;
        return Ok(Err(TemplateDbError::ProjectArchived));
    }
    if !access.permission.at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(TemplateDbError::NotFound));
    }
    let template = match find_template(&mut tx, workspace_id, template_id, kinds).await? {
        Some(row) => row,
        None => {
            tx.rollback().await?;
            return Ok(Err(TemplateDbError::NotFound));
        }
    };
    let project_key = if let Some(project_id) = input.project_id {
        let locked = lock_project(&mut tx, workspace_id, project_id).await?;
        let Some(locked) = locked else {
            tx.rollback().await?;
            return Ok(Err(TemplateDbError::NotFound));
        };
        Some((project_id, locked.key, locked.root_document_id))
    } else {
        None
    };
    tx.commit().await?;

    let title = resolved_title(&template);
    if !title_is_valid(&title) {
        return Ok(Err(TemplateDbError::InvalidInput));
    }

    match template.kind {
        TemplateKind::Document => {
            let parent_id = input
                .parent_id
                .or_else(|| project_key.as_ref().and_then(|(_, _, root)| *root));
            if let Some((project_id, project_key_str, _)) = project_key {
                let meta = match create_project_document(
                    pool,
                    workspace_id,
                    project_id,
                    actor.user_id,
                    session_id,
                    CreateDocumentInput {
                        parent_id,
                        title: &title,
                        icon: None,
                    },
                    client_ip,
                )
                .await?
                {
                    Ok(meta) => meta,
                    Err(DocumentDbError::Forbidden) => return Ok(Err(TemplateDbError::Forbidden)),
                    Err(DocumentDbError::NotFound) => return Ok(Err(TemplateDbError::NotFound)),
                    Err(DocumentDbError::AffiliationMismatch) => {
                        return Ok(Err(TemplateDbError::NotFound));
                    }
                    Err(_) => return Ok(Err(TemplateDbError::InvalidInput)),
                };
                return Ok(Ok(AppliedTemplate {
                    kind: TemplateKind::Document,
                    id: meta.id,
                    display_id: format_display_id(&project_key_str, meta.number),
                }));
            }
            let meta = match create_wiki_document(
                pool,
                workspace_id,
                actor.user_id,
                session_id,
                CreateDocumentInput {
                    parent_id,
                    title: &title,
                    icon: None,
                },
                client_ip,
            )
            .await?
            {
                Ok(meta) => meta,
                Err(DocumentDbError::Forbidden) => return Ok(Err(TemplateDbError::Forbidden)),
                Err(DocumentDbError::NotFound) => return Ok(Err(TemplateDbError::NotFound)),
                Err(DocumentDbError::AffiliationMismatch) => {
                    return Ok(Err(TemplateDbError::NotFound));
                }
                Err(_) => return Ok(Err(TemplateDbError::InvalidInput)),
            };
            let display_id = meta
                .display_id
                .clone()
                .unwrap_or_else(|| format_display_id("WIKI", meta.number));
            Ok(Ok(AppliedTemplate {
                kind: TemplateKind::Document,
                id: meta.id,
                display_id,
            }))
        }
        TemplateKind::Task => {
            let Some((project_id, key, _)) = project_key else {
                return Ok(Err(TemplateDbError::InvalidInput));
            };
            let created = match create_task(
                pool,
                workspace_id,
                project_id,
                actor.user_id,
                session_id,
                CreateTaskInput {
                    title: &title,
                    task_type: "task",
                    priority: "none",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: input.parent_id,
                    milestone_id: None,
                    recurrence: None,
                },
                client_ip,
                channel,
            )
            .await?
            {
                Ok(row) => row,
                Err(ProjectDbError::Forbidden) => return Ok(Err(TemplateDbError::Forbidden)),
                Err(ProjectDbError::Archived) => return Ok(Err(TemplateDbError::ProjectArchived)),
                Err(ProjectDbError::NotFound) => return Ok(Err(TemplateDbError::NotFound)),
                Err(_) => return Ok(Err(TemplateDbError::InvalidInput)),
            };
            Ok(Ok(AppliedTemplate {
                kind: TemplateKind::Task,
                id: created.id,
                display_id: format_display_id(&key, created.number),
            }))
        }
    }
}

/// Wiki creation on every backend without a command receipt: the family
/// writer runs the shared operation and commits only a created document.
async fn create_wiki_document_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_wiki_document(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            input,
            client_ip,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        match create_wiki_document_operation(
            &mut op,
            workspace_id,
            actor_user_id,
            session_id,
            input,
            client_ip,
            None,
        )
        .await?
        {
            Ok(Some(meta)) => Ok(Ok(meta)),
            Ok(None) => Err(sqlx::Error::Protocol(
                "unfenced wiki create lost its target".into(),
            )),
            Err(err) => Ok(Err(err)),
        }
    }
    .await;
    crate::db::document_ops::finish_document_operation(tx, result).await
}

/// [`apply_template`] for every backend: the same check transaction (member,
/// scope, archive, Edit, template) and then the same creating operation,
/// each in its own reserved family writer.
pub async fn apply_template_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    session_id: Uuid,
    template_id: Uuid,
    input: ApplyTemplateInput,
    kinds: &[TemplateKind],
    channel: &str,
    client_ip: Option<&str>,
) -> TemplateResult<AppliedTemplate> {
    if let Backend::Postgres(pool) = backend {
        return apply_template(
            pool,
            workspace_id,
            actor,
            session_id,
            template_id,
            input,
            kinds,
            channel,
            client_ip,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let checked = async {
        let mut op = tx.operation();
        let Some(role) = begin_member_family(&mut op, workspace_id, actor, true).await? else {
            return Ok(Err(TemplateDbError::NotFound));
        };
        let (archived, permission, project) = match input.project_id {
            None => (false, workspace_base_permission(role), None),
            Some(project_id) => {
                let OperationTx::SqliteFamily(family) = &mut op else {
                    unreachable!()
                };
                family.require_tenant(workspace_id)?;
                let rows = family
                    .query(
                        "SELECT status,key,root_document_id FROM projects
                         WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                        &[Cell::uuid(workspace_id), Cell::uuid(project_id)],
                    )
                    .await?;
                let Some(row) = rows.first() else {
                    return Ok(Err(TemplateDbError::NotFound));
                };
                let (status, key, root) = (
                    row.cell(0)?.string()?,
                    row.cell(1)?.string()?,
                    row.cell(2)?.optional(Cell::id)?,
                );
                let permission = op
                    .project_permission_by_id(workspace_id, actor.user_id, project_id)
                    .await?
                    .unwrap_or(ProjectPermission::None);
                (
                    status == "archived",
                    permission,
                    Some((project_id, key, root)),
                )
            }
        };
        if archived {
            return Ok(Err(TemplateDbError::ProjectArchived));
        }
        if !permission.at_least(ProjectPermission::Edit) || kinds.is_empty() {
            return Ok(Err(TemplateDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let [first, second] = family_kind_cells(kinds);
        let rows = family
            .query(
                "SELECT id,workspace_id,kind,title,payload,created_by,created_at,updated_at
                 FROM templates WHERE workspace_id=?1 AND id=?2 AND kind IN (?3,?4)",
                &[
                    Cell::uuid(workspace_id),
                    Cell::uuid(template_id),
                    first,
                    second,
                ],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(TemplateDbError::NotFound));
        };
        Ok(Ok((family_template_row(row)?, project)))
    }
    .await;
    let cleanup = tx.rollback().await;
    let (template, project_key) = match template_read_after_rollback(checked, cleanup)? {
        Ok(checked) => checked,
        Err(err) => return Ok(Err(err)),
    };

    let title = resolved_title(&template);
    if !title_is_valid(&title) {
        return Ok(Err(TemplateDbError::InvalidInput));
    }

    match template.kind {
        TemplateKind::Document => {
            let parent_id = input
                .parent_id
                .or_else(|| project_key.as_ref().and_then(|(_, _, root)| *root));
            let document = CreateDocumentInput {
                parent_id,
                title: &title,
                icon: None,
            };
            let (created, project_key) = match project_key {
                Some((project_id, key, _)) => (
                    create_project_document_backend(
                        backend,
                        workspace_id,
                        project_id,
                        actor.user_id,
                        session_id,
                        document,
                        client_ip,
                    )
                    .await?,
                    Some(key),
                ),
                None => (
                    create_wiki_document_backend(
                        backend,
                        workspace_id,
                        actor.user_id,
                        session_id,
                        document,
                        client_ip,
                    )
                    .await?,
                    None,
                ),
            };
            let meta = match created {
                Ok(meta) => meta,
                Err(DocumentDbError::Forbidden) => return Ok(Err(TemplateDbError::Forbidden)),
                Err(DocumentDbError::NotFound) => return Ok(Err(TemplateDbError::NotFound)),
                Err(DocumentDbError::AffiliationMismatch) => {
                    return Ok(Err(TemplateDbError::NotFound));
                }
                Err(_) => return Ok(Err(TemplateDbError::InvalidInput)),
            };
            let display_id = match project_key {
                Some(key) => format_display_id(&key, meta.number),
                None => meta
                    .display_id
                    .clone()
                    .unwrap_or_else(|| format_display_id("WIKI", meta.number)),
            };
            Ok(Ok(AppliedTemplate {
                kind: TemplateKind::Document,
                id: meta.id,
                display_id,
            }))
        }
        TemplateKind::Task => {
            let Some((project_id, key, _)) = project_key else {
                return Ok(Err(TemplateDbError::InvalidInput));
            };
            let created = match create_task_backend(
                backend,
                workspace_id,
                project_id,
                actor.user_id,
                session_id,
                CreateTaskInput {
                    title: &title,
                    task_type: "task",
                    priority: "none",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: input.parent_id,
                    milestone_id: None,
                    recurrence: None,
                },
                client_ip,
                channel,
            )
            .await?
            {
                Ok(row) => row,
                Err(ProjectDbError::Forbidden) => return Ok(Err(TemplateDbError::Forbidden)),
                Err(ProjectDbError::Archived) => return Ok(Err(TemplateDbError::ProjectArchived)),
                Err(ProjectDbError::NotFound) => return Ok(Err(TemplateDbError::NotFound)),
                Err(_) => return Ok(Err(TemplateDbError::InvalidInput)),
            };
            Ok(Ok(AppliedTemplate {
                kind: TemplateKind::Task,
                id: created.id,
                display_id: format_display_id(&key, created.number),
            }))
        }
    }
}
