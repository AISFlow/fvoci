//! Workspace document tags and their assignment to wiki/project documents.
//!
//! Source `packages/core/src/document-tag.ts`: members create tags, admins
//! rename/recolor/delete them; assigning needs edit access to the document
//! (and a writable project), listing a document's tags needs view access.

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::collections::AttachTarget;
use crate::db::collections::{begin_member, require_target, Actor, CollectionDbError, Need};
use crate::db::identity::{append_audit, AuditAppend};
use crate::db::workspace::WorkspaceRole;

pub const TAG_POOL_LIMIT_MAX: i64 = 100;
pub const TAG_POOL_LIMIT_DEFAULT: i64 = 50;
pub const TAG_QUERY_MAX: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagDbError {
    NotFound,
    Forbidden,
    Conflict,
    ProjectArchived,
}

pub type TagResult<T> = Result<Result<T, TagDbError>, sqlx::Error>;

#[derive(Debug, Clone)]
pub struct TagRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub color: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub assignment_count: i64,
}

#[derive(Debug, Clone)]
pub struct TagPool {
    pub can_create: bool,
    pub can_manage: bool,
    pub items: Vec<TagRow>,
}

/// Which document route was used: the document must live there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affiliation {
    Wiki,
    Project(Uuid),
}

fn from_collection_error(err: CollectionDbError) -> TagDbError {
    match err {
        CollectionDbError::Forbidden => TagDbError::Forbidden,
        CollectionDbError::ProjectArchived | CollectionDbError::TaskArchived => {
            TagDbError::ProjectArchived
        }
        _ => TagDbError::NotFound,
    }
}

type TagTuple = (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>);

fn tag_from(row: TagTuple, assignment_count: i64) -> TagRow {
    TagRow {
        id: row.0,
        workspace_id: row.1,
        name: row.2,
        color: row.3,
        created_at: row.4,
        updated_at: row.5,
        assignment_count,
    }
}

type PoolTuple = (
    Uuid,
    Uuid,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    i64,
);

const TAG_COLUMNS: &str = "id, workspace_id, name, color, created_at, updated_at";

fn at_least(role: WorkspaceRole, min: WorkspaceRole) -> bool {
    let rank = |role: WorkspaceRole| match role {
        WorkspaceRole::Guest => 0,
        WorkspaceRole::Member => 1,
        WorkspaceRole::Admin => 2,
        WorkspaceRole::Owner => 3,
    };
    rank(role) >= rank(min)
}

macro_rules! member {
    ($tx:expr, $ws:expr, $actor:expr, $write:expr) => {
        match begin_member(&mut $tx, $ws, $actor, $write).await? {
            Ok(role) => role,
            Err(_) => {
                $tx.rollback().await?;
                return Ok(Err(TagDbError::NotFound));
            }
        }
    };
}

async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    verb: &str,
    tag_id: Uuid,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor.user_id),
            verb: verb.to_string(),
            target_type: Some("document_tag".to_string()),
            target_id: Some(tag_id),
            payload,
            ip: actor.client_ip.map(|ip| ip.to_string()),
        },
    )
    .await
}

pub async fn list_tags(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    q: Option<&str>,
    limit: i64,
) -> TagResult<TagPool> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, false);
    let q = q.map(str::trim).filter(|q| !q.is_empty());
    let rows: Vec<PoolTuple> = sqlx::query_as(
        r#"
            SELECT t.id, t.workspace_id, t.name, t.color, t.created_at, t.updated_at,
                   (SELECT count(*) FROM fvoci.document_tag_assignments a
                    WHERE a.workspace_id = t.workspace_id AND a.tag_id = t.id)
            FROM fvoci.document_tags t
            WHERE t.workspace_id = $1
              AND ($2::text IS NULL OR strpos(lower(t.name), lower($2)) > 0)
            ORDER BY t.name, t.id
            LIMIT $3
            "#,
    )
    .bind(workspace_id)
    .bind(q)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(TagPool {
        can_create: at_least(role, WorkspaceRole::Member),
        can_manage: at_least(role, WorkspaceRole::Admin),
        items: rows
            .into_iter()
            .map(|r| tag_from((r.0, r.1, r.2, r.3, r.4, r.5), r.6))
            .collect(),
    }))
}

pub async fn create_tag(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    name: &str,
    color: &str,
) -> TagResult<TagRow> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, true);
    if !at_least(role, WorkspaceRole::Member) {
        tx.rollback().await?;
        return Ok(Err(TagDbError::Forbidden));
    }
    let row: Option<TagTuple> = sqlx::query_as(&format!(
        "INSERT INTO fvoci.document_tags (id, workspace_id, name, color) VALUES ($1, $2, $3, $4) \
         ON CONFLICT DO NOTHING RETURNING {TAG_COLUMNS}"
    ))
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(name)
    .bind(color)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(Err(TagDbError::Conflict));
    };
    let tag = tag_from(row, 0);
    audit(
        &mut tx,
        workspace_id,
        actor,
        "document_tag.created",
        tag.id,
        json!({"name": tag.name, "color": tag.color}),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(tag))
}

pub async fn update_tag(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    tag_id: Uuid,
    name: Option<&str>,
    color: Option<&str>,
) -> TagResult<TagRow> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, true);
    if !at_least(role, WorkspaceRole::Admin) {
        tx.rollback().await?;
        return Ok(Err(TagDbError::Forbidden));
    }
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM fvoci.document_tags WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(tag_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(Err(TagDbError::NotFound));
    }
    if let Some(name) = name {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fvoci.document_tags WHERE workspace_id = $1 AND lower(name) = lower($2) AND id <> $3)",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(tag_id)
        .fetch_one(&mut *tx)
        .await?;
        if taken {
            tx.rollback().await?;
            return Ok(Err(TagDbError::Conflict));
        }
    }
    let updated = sqlx::query_as::<_, TagTuple>(&format!(
        "UPDATE fvoci.document_tags SET name = COALESCE($3, name), color = COALESCE($4, color), updated_at = now() \
         WHERE workspace_id = $1 AND id = $2 RETURNING {TAG_COLUMNS}"
    ))
    .bind(workspace_id)
    .bind(tag_id)
    .bind(name)
    .bind(color)
    .fetch_one(&mut *tx)
    .await;
    let row = match updated {
        Ok(row) => row,
        // A concurrent rename took the name between the check and the write.
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("23505") => {
            tx.rollback().await?;
            return Ok(Err(TagDbError::Conflict));
        }
        Err(err) => return Err(err),
    };
    let tag = tag_from(row, 0);
    audit(
        &mut tx,
        workspace_id,
        actor,
        "document_tag.updated",
        tag.id,
        json!({"name": tag.name, "color": tag.color}),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(tag))
}

pub async fn delete_tag(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    tag_id: Uuid,
) -> TagResult<()> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, true);
    if !at_least(role, WorkspaceRole::Admin) {
        tx.rollback().await?;
        return Ok(Err(TagDbError::Forbidden));
    }
    let deleted: Option<String> = sqlx::query_scalar(
        "DELETE FROM fvoci.document_tags WHERE workspace_id = $1 AND id = $2 RETURNING name",
    )
    .bind(workspace_id)
    .bind(tag_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(name) = deleted else {
        tx.rollback().await?;
        return Ok(Err(TagDbError::NotFound));
    };
    audit(
        &mut tx,
        workspace_id,
        actor,
        "document_tag.deleted",
        tag_id,
        json!({"name": name}),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

async fn require_document(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    role: WorkspaceRole,
    document_id: Uuid,
    affiliation: Affiliation,
    need: Need,
) -> TagResult<()> {
    let info = match require_target(
        tx,
        workspace_id,
        actor,
        role,
        AttachTarget::Document(document_id),
        need,
    )
    .await?
    {
        Ok(info) => info,
        Err(err) => return Ok(Err(from_collection_error(err))),
    };
    let matches = match affiliation {
        Affiliation::Wiki => info.project_id.is_none(),
        Affiliation::Project(project_id) => info.project_id == Some(project_id),
    };
    Ok(if matches {
        Ok(())
    } else {
        Err(TagDbError::NotFound)
    })
}

pub async fn list_document_tags(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    document_id: Uuid,
    affiliation: Affiliation,
) -> TagResult<Vec<TagRow>> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, false);
    if let Err(err) = require_document(
        &mut tx,
        workspace_id,
        actor,
        role,
        document_id,
        affiliation,
        Need::Read,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let rows: Vec<TagTuple> = sqlx::query_as(
        r#"
        SELECT t.id, t.workspace_id, t.name, t.color, t.created_at, t.updated_at
        FROM fvoci.document_tag_assignments a
        JOIN fvoci.document_tags t ON t.workspace_id = a.workspace_id AND t.id = a.tag_id
        WHERE a.workspace_id = $1 AND a.document_id = $2
        ORDER BY t.name, t.id
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows.into_iter().map(|row| tag_from(row, 0)).collect()))
}

pub async fn assign_tag(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    document_id: Uuid,
    affiliation: Affiliation,
    tag_id: Uuid,
) -> TagResult<TagRow> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, true);
    if let Err(err) = require_document(
        &mut tx,
        workspace_id,
        actor,
        role,
        document_id,
        affiliation,
        Need::Edit,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let tag: Option<TagTuple> = sqlx::query_as(&format!(
        "SELECT {TAG_COLUMNS} FROM fvoci.document_tags WHERE workspace_id = $1 AND id = $2 FOR SHARE"
    ))
    .bind(workspace_id)
    .bind(tag_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(tag) = tag else {
        tx.rollback().await?;
        return Ok(Err(TagDbError::NotFound));
    };
    sqlx::query(
        "INSERT INTO fvoci.document_tag_assignments (workspace_id, document_id, tag_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(tag_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(tag_from(tag, 0)))
}

pub async fn unassign_tag(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    document_id: Uuid,
    affiliation: Affiliation,
    tag_id: Uuid,
) -> TagResult<()> {
    let mut tx = pool.begin().await?;
    let role = member!(tx, workspace_id, actor, true);
    if let Err(err) = require_document(
        &mut tx,
        workspace_id,
        actor,
        role,
        document_id,
        affiliation,
        Need::Edit,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let removed = sqlx::query(
        "DELETE FROM fvoci.document_tag_assignments WHERE workspace_id = $1 AND document_id = $2 AND tag_id = $3",
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(tag_id)
    .execute(&mut *tx)
    .await?;
    if removed.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(TagDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Tree `?tag=` filter: ids of documents carrying the tag.
pub(crate) async fn tagged_document_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    tag_id: Uuid,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT document_id FROM fvoci.document_tag_assignments WHERE workspace_id = $1 AND tag_id = $2",
    )
    .bind(workspace_id)
    .bind(tag_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(ids.into_iter().collect())
}

/// Pool form of [`tagged_document_ids`] for the tree routes: the tree itself
/// already applied the actor's read access, the tag only narrows it.
pub async fn tagged_document_id_set(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    tag_id: Uuid,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, workspace_id).await?;
    let ids = tagged_document_ids(&mut tx, workspace_id, tag_id).await?;
    tx.commit().await?;
    Ok(ids)
}
