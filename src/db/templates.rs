//! Workspace document/task templates (source `packages/core/src/template.ts`).

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::collections::{begin_member, scope_access, Actor, CollectionDbError};
use crate::db::context::set_tenant;
use crate::db::documents::{
    create_wiki_document, CreateDocumentInput, DocumentDbError,
};
use crate::db::project_documents::create_project_document;
use crate::db::projects::{lock_project, ProjectDbError};
use crate::db::tasks::{create_task, CreateTaskInput};
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

pub fn title_is_valid(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 200
}

/// Bounded payload: JSON object; optional `title` must be a string when present.
pub fn normalize_payload(payload: Value) -> Result<Value, TemplateDbError> {
    let Some(obj) = payload.as_object() else {
        return Err(TemplateDbError::InvalidInput);
    };
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

pub async fn list_templates(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    kinds: &[TemplateKind],
) -> TemplateResult<Vec<TemplateRow>> {
    if kinds.is_empty() {
        return Ok(Ok(vec![]));
    }
    let kind_filter: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
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
    Ok(row.and_then(|r| {
        row_from_tuple(r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7)
    }))
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
