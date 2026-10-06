//! Workspace document tags and their assignment to wiki/project documents.
//!
//! Source `packages/core/src/document-tag.ts`: members create tags, admins
//! rename/recolor/delete them; assigning needs edit access to the document
//! (and a writable project), listing a document's tags needs view access.

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

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

/// Selected tag-pool display/search keeps PG's locale-sensitive entrypoint.
/// Family search uses existing Rust Unicode lowercase, BINARY name/UUID ties,
/// and bounds each fetched page without limiting the searched workspace pool.
pub async fn list_tags_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    q: Option<&str>,
    limit: i64,
) -> TagResult<TagPool> {
    if let Backend::Postgres(pool) = backend {
        return list_tags(pool, workspace_id, actor, q, limit).await;
    }
    if !(1..=TAG_POOL_LIMIT_MAX).contains(&limit) {
        return Err(sqlx::Error::Protocol("invalid tag pool limit".into()));
    }
    let limit = usize::try_from(limit)
        .map_err(|_| sqlx::Error::Protocol("invalid tag pool limit".into()))?;
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op
            .session_is_live(actor.user_id, actor.credential_id)
            .await?
            || !op.workspace_is_live(workspace_id).await?
        {
            return Ok(Err(TagDbError::NotFound));
        }
        let Some(role) = op
            .membership_role(workspace_id, actor.user_id, false)
            .await?
        else {
            return Ok(Err(TagDbError::NotFound));
        };
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family.require_tenant(workspace_id)?;
        let needle = q
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_lowercase);
        let mut after: Option<(String, Uuid)> = None;
        let mut items = Vec::new();
        loop {
            let rows = family
                .query(
                    "SELECT t.id,t.workspace_id,t.name,t.color,t.created_at,t.updated_at,
                        (SELECT count(*) FROM document_tag_assignments a
                         WHERE a.workspace_id=t.workspace_id AND a.tag_id=t.id)
                 FROM document_tags t WHERE t.workspace_id=?1
                   AND (?2 IS NULL OR t.name COLLATE BINARY>?2
                        OR (t.name COLLATE BINARY=?2 AND t.id>?3))
                 ORDER BY t.name COLLATE BINARY,t.id LIMIT 128",
                    &[
                        Cell::uuid(workspace_id),
                        Cell::optional_text(after.as_ref().map(|(name, _)| name.as_str())),
                        Cell::optional_uuid(after.as_ref().map(|(_, id)| *id)),
                    ],
                )
                .await?;
            for row in &rows {
                let mut tag = family_tag_row(row)?;
                after = Some((tag.name.clone(), tag.id));
                if needle
                    .as_ref()
                    .is_some_and(|needle| !tag.name.to_lowercase().contains(needle))
                {
                    continue;
                }
                tag.assignment_count = row.cell(6)?.integer()?;
                items.push(tag);
                if items.len() == limit {
                    break;
                }
            }
            if items.len() == limit || rows.len() < 128 {
                break;
            }
        }
        Ok(Ok(TagPool {
            can_create: at_least(role, WorkspaceRole::Member),
            can_manage: at_least(role, WorkspaceRole::Admin),
            items,
        }))
    }
    .await;
    let cleanup = tx.rollback().await;
    tag_pool_read_after_rollback(result, cleanup)
}

fn tag_pool_read_after_rollback(
    result: TagResult<TagPool>,
    cleanup: Result<(), sqlx::Error>,
) -> TagResult<TagPool> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(TagReadRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
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

/// The PG entry point retains its locale-sensitive lower(name) index. Selected
/// SQLite-family creation additionally compares Rust std Unicode lowercase in
/// its reserved writer: no normalization, full case folding or PG locale claim.
pub async fn create_tag_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    name: &str,
    color: &str,
) -> TagResult<TagRow> {
    if let Backend::Postgres(pool) = backend {
        return create_tag(pool, workspace_id, actor, name, color).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        let role = match tag_write_member(&mut op, workspace_id, actor).await? {
            Ok(role) => role,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if !at_least(role, WorkspaceRole::Member) {
            return Ok(Err(TagDbError::Forbidden));
        }
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        if family_tag_name_taken(family, workspace_id, name).await? {
            return Ok(Err(TagDbError::Conflict));
        }
        let rows = family
            .query(
                "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,?3,?4)
             ON CONFLICT DO NOTHING RETURNING id,workspace_id,name,color,created_at,updated_at",
                &[
                    Cell::uuid(Uuid::now_v7()),
                    Cell::uuid(workspace_id),
                    Cell::text(name),
                    Cell::text(color),
                ],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(TagDbError::Conflict));
        };
        let tag = family_tag_row(row)?;
        op.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor.user_id),
            verb: "document_tag.created".into(),
            target_type: Some("document_tag".into()),
            target_id: Some(tag.id),
            payload: json!({"name":tag.name,"color":tag.color}),
            ip: actor.client_ip.map(|ip| ip.to_string()),
        })
        .await?;
        Ok(Ok(tag))
    }
    .await;
    finish_tag_write(tx, result).await
}

async fn tag_write_member(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: &Actor,
) -> TagResult<WorkspaceRole> {
    op.set_tenant(workspace).await?;
    op.lock_membership_users(&[actor.user_id]).await?;
    if !op
        .recheck_session(actor.user_id, actor.credential_id)
        .await?
        || !op.workspace_is_live(workspace).await?
    {
        return Ok(Err(TagDbError::NotFound));
    }
    Ok(op
        .membership_role(workspace, actor.user_id, true)
        .await?
        .ok_or(TagDbError::NotFound))
}

async fn family_tag_name_taken(
    family: &mut FamilyTx,
    workspace: Uuid,
    name: &str,
) -> Result<bool, sqlx::Error> {
    family.require_writer()?;
    family.require_tenant(workspace)?;
    let lowercase = name.to_lowercase();
    let mut after = None;
    loop {
        // Bound each fetched page; never cap the workspace's tag count. The
        // current writer keeps this scan and INSERT in one serialized snapshot.
        let rows = family
            .query(
                "SELECT id,name FROM document_tags WHERE workspace_id=?1
             AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT 128",
                &[Cell::uuid(workspace), Cell::optional_uuid(after)],
            )
            .await?;
        for row in &rows {
            if row.cell(1)?.string()?.to_lowercase() == lowercase {
                return Ok(true);
            }
            after = Some(row.cell(0)?.id()?);
        }
        if rows.len() < 128 {
            return Ok(false);
        }
    }
}

fn family_tag_row(row: &FamilyRow) -> Result<TagRow, sqlx::Error> {
    Ok(tag_from(
        (
            row.cell(0)?.id()?,
            row.cell(1)?.id()?,
            row.cell(2)?.string()?,
            row.cell(3)?.string()?,
            row.cell(4)?.datetime()?,
            row.cell(5)?.datetime()?,
        ),
        0,
    ))
}

#[derive(Debug, thiserror::Error)]
#[error("tag write refused: {0:?}")]
struct TagWriteRefusal(TagDbError);

async fn finish_tag_write(tx: DbTx, result: TagResult<TagRow>) -> TagResult<TagRow> {
    match result {
        Ok(Ok(tag)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(tag))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(TagWriteRefusal(refusal))),
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

/// Assigned display tags. Family name ordering is native BINARY with UUID
/// ties; stored Unicode strings are neither folded nor normalized. This narrow
/// display contract does not change PG locale order, name uniqueness or cursors.
pub async fn list_document_tags_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    document_id: Uuid,
    affiliation: Affiliation,
) -> TagResult<Vec<TagRow>> {
    if let Backend::Postgres(pool) = backend {
        return list_document_tags(pool, workspace_id, actor, document_id, affiliation).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op.session_is_live(actor.user_id,actor.credential_id).await?
            || !op.workspace_is_live(workspace_id).await?
            || op.membership_role(workspace_id,actor.user_id,false).await?.is_none()
        { return Ok(Err(TagDbError::NotFound)); }
        let Some(document) = op.document_row(workspace_id,document_id).await? else { return Ok(Err(TagDbError::NotFound)); };
        // DocumentRow stores project_id in its ninth tuple field.
        let project_id = document.8;
        let matches = match affiliation { Affiliation::Wiki => project_id.is_none(),Affiliation::Project(project) => project_id==Some(project) };
        if !matches { return Ok(Err(TagDbError::NotFound)); }
        let permission = if let Some(project) = project_id {
            op.project_permission_by_id(workspace_id,actor.user_id,project).await?.unwrap_or(crate::projects::ProjectPermission::None)
        } else { op.document_permission(workspace_id,actor.user_id,document_id,true).await? };
        if !permission.at_least(crate::projects::ProjectPermission::View) { return Ok(Err(TagDbError::NotFound)); }
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        family.require_tenant(workspace_id)?;
        let rows = family.query("SELECT t.id,t.workspace_id,t.name,t.color,t.created_at,t.updated_at
            FROM document_tag_assignments a JOIN document_tags t ON t.workspace_id=a.workspace_id AND t.id=a.tag_id
            WHERE a.workspace_id=?1 AND a.document_id=?2 ORDER BY t.name,t.id", &[Cell::uuid(workspace_id),Cell::uuid(document_id)]).await?;
        let tags = rows.iter().map(|row| Ok(tag_from((row.cell(0)?.id()?,row.cell(1)?.id()?,row.cell(2)?.string()?,row.cell(3)?.string()?,row.cell(4)?.datetime()?,row.cell(5)?.datetime()?),0))).collect::<Result<Vec<_>,sqlx::Error>>()?;
        Ok(Ok(tags))
    }.await;
    match result {
        Ok(Ok(tags)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(tags))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(TagReadRefusal(refusal))),
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

#[derive(Debug, thiserror::Error)]
#[error("tag read refused: {0:?}")]
struct TagReadRefusal(TagDbError);

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

pub async fn assign_tag_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    document_id: Uuid,
    affiliation: Affiliation,
    tag_id: Uuid,
) -> TagResult<TagRow> {
    if let Backend::Postgres(pool) = backend {
        return assign_tag(pool, workspace_id, actor, document_id, affiliation, tag_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if let Err(refusal) = tag_write_member(&mut op,workspace_id,actor).await? {
            return Ok(Err(refusal));
        }
        op.lock_tree(workspace_id).await?;
        let Some(document) = op.document_row(workspace_id,document_id).await? else {
            return Ok(Err(TagDbError::NotFound));
        };
        let (permission,archived) = if let Some(project) = document.8 {
            op.share_lock_project_permission(workspace_id,actor.user_id,project).await?
                .unwrap_or((crate::projects::ProjectPermission::None,false))
        } else {
            (op.document_permission(workspace_id,actor.user_id,document_id,true).await?,false)
        };
        if !permission.at_least(crate::projects::ProjectPermission::View) {
            return Ok(Err(TagDbError::NotFound));
        }
        if !permission.at_least(crate::projects::ProjectPermission::Edit) {
            return Ok(Err(TagDbError::Forbidden));
        }
        if archived {
            return Ok(Err(TagDbError::ProjectArchived));
        }
        let matches = match affiliation {
            Affiliation::Wiki => document.8.is_none(),
            Affiliation::Project(project) => document.8==Some(project),
        };
        if !matches { return Ok(Err(TagDbError::NotFound)); }
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        family.require_writer()?;
        family.require_tenant(workspace_id)?;
        let rows = family.query(
            "SELECT id,workspace_id,name,color,created_at,updated_at FROM document_tags WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(workspace_id),Cell::uuid(tag_id)],
        ).await?;
        let Some(row) = rows.first() else { return Ok(Err(TagDbError::NotFound)); };
        let tag = family_tag_row(row)?;
        family.execute(
            "INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
            &[Cell::uuid(workspace_id),Cell::uuid(document_id),Cell::uuid(tag_id)],
        ).await?;
        Ok(Ok(tag))
    }.await;
    finish_tag_write(tx, result).await
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

#[cfg(all(test, feature = "db-tests"))]
mod selected_assigned_tag_read_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    #[tokio::test]
    async fn wiki_aux_tags_selected_unicode_tenant_grant_affiliation_and_healthy_read() {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'tags-read',?3)",
        )
        .bind(credential.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .bind(chrono::Utc::now().timestamp_micros() + 86_400_000_000)
        .execute(&f.pool)
        .await
        .unwrap();
        let actor = Actor {
            user_id: f.user,
            credential_id: credential,
            client_ip: None,
        };
        let names = ["가", "가", "中", "e\u{301}", "A", "😀"];
        let ids = names
            .iter()
            .enumerate()
            .map(|(i, _)| Uuid::from_u128(200 + i as u128))
            .collect::<Vec<_>>();
        for (name, id) in names.iter().zip(&ids) {
            sqlx::query(
                "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,?3,'violet')",
            )
            .bind(id.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(*name)
            .execute(&f.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3)").bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        }
        let rows = list_document_tags_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Wiki,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "e\u{301}", "가", "中", "가", "😀"]
        );
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[4], ids[3], ids[1], ids[2], ids[0], ids[5]]
        );
        assert!(rows
            .iter()
            .all(|row| row.workspace_id == f.workspace && row.color == "violet"));
        // Equal-name rows in one tenant are forbidden by the retained catalog;
        // do not drop its unique index merely to manufacture an ordering tie.
        assert!(sqlx::query(
            "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'가','red')"
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(f.workspace.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .is_err());
        let other = Uuid::now_v7();
        let doc = Uuid::now_v7();
        let tag = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'tags-other','Other')")
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(other.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other',?3,'V',1,'published',2,?4,'{}')").bind(doc.as_bytes().as_slice()).bind(other.as_bytes().as_slice()).bind(doc.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query(
            "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'가','red')",
        )
        .bind(tag.as_bytes().as_slice())
        .bind(other.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3)").bind(other.as_bytes().as_slice()).bind(doc.as_bytes().as_slice()).bind(tag.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_document_tags_backend(&f.backend, other, &actor, doc, Affiliation::Wiki)
                .await
                .unwrap()
                .unwrap()[0]
                .id,
            tag
        );
        assert!(matches!(
            list_document_tags_backend(&f.backend, other, &actor, f.document, Affiliation::Wiki)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        assert!(matches!(
            list_document_tags_backend(&f.backend, f.workspace, &actor, doc, Affiliation::Wiki)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        assert!(matches!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Project(Uuid::now_v7())
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Wiki viewers')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            6
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki
            )
            .await
            .unwrap()
            .unwrap()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
            rows.iter().map(|row| row.id).collect::<Vec<_>>()
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_tag_write_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn actor(f: &Fixture) -> Actor {
        let credential = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                "tag-write-session",
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Actor {
            user_id: f.user,
            credential_id: credential,
            client_ip: Some("127.0.0.1".parse().unwrap()),
        }
    }

    async fn effects(f: &Fixture) -> (i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM document_tags),(SELECT count(*) FROM document_tag_assignments),(SELECT count(*) FROM audit_log),(SELECT count(*) FROM events)")
            .fetch_one(&f.pool).await.unwrap()
    }

    #[tokio::test]
    async fn wiki_aux_mutation_tag_unicode_pages_conflicts_canonical_controls_and_concurrent_writers(
    ) {
        let f = Fixture::new().await;
        let actor = actor(&f).await;
        // The last name is outside the first bounded page; no early-page pass
        // may authorize its duplicate. These rows exercise the retained schema.
        for i in 0..130_u128 {
            let name = if i == 129 {
                "Boundary É".into()
            } else {
                format!("fixture-{i:03}")
            };
            sqlx::query(
                "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,?3,'gray')",
            )
            .bind(Uuid::from_u128(1000 + i).as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(name)
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let before = effects(&f).await;
        assert!(matches!(
            create_tag_backend(&f.backend, f.workspace, &actor, "boundary é", "blue")
                .await
                .unwrap(),
            Err(TagDbError::Conflict)
        ));
        assert_eq!(effects(&f).await, before);
        for (name, duplicate) in [
            ("ASCII", "ascii"),
            ("ÉCOLE", "école"),
            ("Σ", "σ"),
            ("ΟΣ", "ος"),
            ("İ", "i\u{307}"),
        ] {
            let row = create_tag_backend(&f.backend, f.workspace, &actor, name, "violet")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.name, name);
            assert_eq!(row.color, "violet");
            let before = effects(&f).await;
            assert!(matches!(
                create_tag_backend(&f.backend, f.workspace, &actor, duplicate, "red")
                    .await
                    .unwrap(),
                Err(TagDbError::Conflict)
            ));
            assert!(matches!(
                create_tag_backend(&f.backend, f.workspace, &actor, name, "red")
                    .await
                    .unwrap(),
                Err(TagDbError::Conflict)
            ));
            assert_eq!(effects(&f).await, before);
        }
        // std lowercase is neither full case folding nor normalization. Dotted
        // I versus i, final sigma versus sigma, and NFC versus NFD stay distinct.
        for name in ["i", "ς", "é", "e\u{301}", "가", "가"] {
            assert_eq!(
                create_tag_backend(&f.backend, f.workspace, &actor, name, "green")
                    .await
                    .unwrap()
                    .unwrap()
                    .name,
                name
            );
        }
        let race_pool = crate::db::pool::connect_sqlite_app(&f.path, 2)
            .await
            .unwrap();
        let race_backend = Backend::Sqlite(race_pool.clone());
        let before = effects(&f).await;
        let (a, b) = tokio::join!(
            create_tag_backend(&race_backend, f.workspace, &actor, "RACE É", "blue"),
            create_tag_backend(&race_backend, f.workspace, &actor, "race é", "red")
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(
            usize::from(matches!(a, Err(TagDbError::Conflict)))
                + usize::from(matches!(b, Err(TagDbError::Conflict))),
            1
        );
        assert_eq!(
            effects(&f).await,
            (before.0 + 1, before.1, before.2 + 1, before.3)
        );
        race_pool.close().await;
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_mutation_tag_authority_affiliation_rollback_and_healthy_assignment() {
        let f = Fixture::new().await;
        let actor = actor(&f).await;
        let tag = create_tag_backend(&f.backend, f.workspace, &actor, "태그 中 😀", "violet")
            .await
            .unwrap()
            .unwrap();
        let before = effects(&f).await;
        let assigned = assign_tag_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Wiki,
            tag.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(assigned.id, tag.id);
        assert_eq!(assigned.assignment_count, 0);
        assert_eq!(
            effects(&f).await,
            (before.0, before.1 + 1, before.2, before.3)
        );
        assign_tag_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Wiki,
            tag.id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            effects(&f).await,
            (before.0, before.1 + 1, before.2, before.3)
        );
        let listed = list_document_tags_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Wiki,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            (
                listed[0].id,
                listed[0].name.as_str(),
                listed[0].color.as_str()
            ),
            (tag.id, "태그 中 😀", "violet")
        );
        let audit: (String, String, String) =
            sqlx::query_as("SELECT verb,payload,ip FROM audit_log WHERE target_id=?1")
                .bind(tag.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(audit.0, "document_tag.created");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&audit.1).unwrap(),
            json!({"name":"태그 中 😀","color":"violet"})
        );
        assert_eq!(audit.2, "127.0.0.1");
        let other_workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'tag-write-other','Other')")
            .bind(other_workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(other_workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other_tag =
            create_tag_backend(&f.backend, other_workspace, &actor, "태그 中 😀", "red")
                .await
                .unwrap()
                .unwrap();
        let baseline = effects(&f).await;
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                other_workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                other_tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                other_tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        let wrong_actor = Actor {
            user_id: Uuid::now_v7(),
            credential_id: actor.credential_id,
            client_ip: None,
        };
        assert!(matches!(
            create_tag_backend(&f.backend, f.workspace, &wrong_actor, "wrong", "red")
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                Uuid::now_v7(),
                &actor,
                f.document,
                Affiliation::Wiki,
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(actor.credential_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_tag_backend(&f.backend, f.workspace, &actor, "revoked", "red")
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(actor.credential_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_tag_backend(&f.backend, f.workspace, &actor, "guest", "red")
                .await
                .unwrap(),
            Err(TagDbError::Forbidden)
        ));
        // A group view grant permits the read, but cannot authorize assignment.
        let viewer_group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Write test viewers')")
            .bind(viewer_group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(viewer_group.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(viewer_group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_document_tags_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki
            )
            .await
            .unwrap()
            .unwrap()[0]
                .id,
            tag.id
        );
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::Forbidden)
        ));
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'TAG','Tag project','workspace',?3)")
            .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Wiki,
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            assign_tag_backend(
                &f.backend,
                f.workspace,
                &actor,
                f.document,
                Affiliation::Project(project),
                tag.id
            )
            .await
            .unwrap(),
            Err(TagDbError::ProjectArchived)
        ));
        assert_eq!(effects(&f).await, baseline);
        sqlx::query("UPDATE projects SET status='active' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assign_tag_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Project(project),
            tag.id,
        )
        .await
        .unwrap()
        .unwrap();
        // A real failure after INSERT must roll back both the tag and its audit.
        sqlx::query("CREATE TRIGGER reject_tag_audit BEFORE INSERT ON audit_log WHEN NEW.verb='document_tag.created' BEGIN SELECT RAISE(ABORT,'tag audit failure'); END;")
            .execute(&f.pool).await.unwrap();
        assert!(
            create_tag_backend(&f.backend, f.workspace, &actor, "healthy retry", "blue")
                .await
                .is_err()
        );
        assert_eq!(effects(&f).await, baseline);
        let missing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM document_tags WHERE name='healthy retry'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(missing, 0);
        sqlx::query("DROP TRIGGER reject_tag_audit")
            .execute(&f.pool)
            .await
            .unwrap();
        let retry = create_tag_backend(&f.backend, f.workspace, &actor, "healthy retry", "blue")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(retry.id, tag.id);
        assert_eq!(
            effects(&f).await,
            (baseline.0 + 1, baseline.1, baseline.2 + 1, baseline.3)
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

#[cfg(test)]
mod selected_tag_pool_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn actor(f: &Fixture) -> Actor {
        let credential_id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential_id,
                f.user,
                "selected-tag-pool",
                crate::db::identity::stored_now()
                    + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Actor {
            user_id: f.user,
            credential_id,
            client_ip: None,
        }
    }

    #[tokio::test]
    async fn sqlite_tag_pool_unicode_search_past_page_limit_and_assignment_counts() {
        let f = Fixture::new().await;
        let actor = actor(&f).await;
        for index in 0..130 {
            sqlx::query(
                "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,?3,'blue')",
            )
            .bind(Uuid::now_v7().as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(format!("a{index:03}"))
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let echo = create_tag_backend(&f.backend, f.workspace, &actor, "Écho", "blue")
            .await
            .unwrap()
            .unwrap();
        let eclair = create_tag_backend(&f.backend, f.workspace, &actor, "Éclair", "green")
            .await
            .unwrap()
            .unwrap();
        let decomposed = create_tag_backend(&f.backend, f.workspace, &actor, "E\u{301}cho", "red")
            .await
            .unwrap()
            .unwrap();
        assign_tag_backend(
            &f.backend,
            f.workspace,
            &actor,
            f.document,
            Affiliation::Wiki,
            echo.id,
        )
        .await
        .unwrap()
        .unwrap();
        let pool = list_tags_backend(&f.backend, f.workspace, &actor, Some(" é "), 2)
            .await
            .unwrap()
            .unwrap();
        assert!(pool.can_create && pool.can_manage);
        assert_eq!(
            pool.items.iter().map(|t| t.id).collect::<Vec<_>>(),
            [echo.id, eclair.id]
        );
        assert_eq!(
            pool.items
                .iter()
                .map(|t| t.assignment_count)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        assert_eq!(pool.items[0].name, "Écho");
        assert_eq!(pool.items[0].color, "blue");
        assert_eq!(pool.items[0].workspace_id, f.workspace);
        assert!(pool.items[0].created_at <= pool.items[0].updated_at);
        assert_eq!(
            list_tags_backend(&f.backend, f.workspace, &actor, Some("É"), 1)
                .await
                .unwrap()
                .unwrap()
                .items[0]
                .id,
            echo.id
        );
        assert_eq!(
            list_tags_backend(&f.backend, f.workspace, &actor, Some("E\u{301}"), 100)
                .await
                .unwrap()
                .unwrap()
                .items[0]
                .id,
            decomposed.id
        );
        let full = list_tags_backend(&f.backend, f.workspace, &actor, Some("  "), 100)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(full.items.len(), 100);
        assert_eq!(full.items[0].name, "E\u{301}cho");
        assert_eq!(full.items[1].name, "a000");
        assert_eq!(full.items[99].name, "a098");
        // Equal names are a raw ordering witness, not an API duplicate-create
        // contract (the selected creator prevents case-equivalent duplicates).
        let tied = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'Écho','blue')",
        )
        .bind(tied.as_bytes().as_slice())
        .bind(f.workspace.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let mut expected = [echo.id, tied];
        expected.sort();
        assert_eq!(
            list_tags_backend(&f.backend, f.workspace, &actor, Some("Écho"), 100)
                .await
                .unwrap()
                .unwrap()
                .items
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            expected
        );
        // Pool counts retain PG's assignment semantics even for a trashed doc.
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list_tags_backend(&f.backend, f.workspace, &actor, Some("Écho"), 1)
                .await
                .unwrap()
                .unwrap()
                .items[0]
                .assignment_count,
            1
        );
        for limit in [0, 101] {
            assert!(
                list_tags_backend(&f.backend, f.workspace, &actor, None, limit)
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_tag_pool_current_authority_fault_and_healthy_retry() {
        let f = Fixture::new().await;
        let actor = actor(&f).await;
        let tag = create_tag_backend(&f.backend, f.workspace, &actor, "Healthy", "blue")
            .await
            .unwrap()
            .unwrap();
        for (role, create, manage) in [
            ("guest", false, false),
            ("member", true, false),
            ("admin", true, true),
        ] {
            sqlx::query("UPDATE memberships SET role=?1 WHERE workspace_id=?2 AND user_id=?3")
                .bind(role)
                .bind(f.workspace.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let pool = list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap()
                .unwrap();
            assert_eq!((pool.can_create, pool.can_manage), (create, manage));
            assert_eq!(pool.items[0].id, tag.id);
        }
        assert!(matches!(
            list_tags_backend(&f.backend, Uuid::now_v7(), &actor, None, 100)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("ALTER TABLE document_tags RENAME COLUMN color TO broken_color")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .is_err()
        );
        sqlx::query("ALTER TABLE document_tags RENAME COLUMN broken_color TO color")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap()
                .unwrap()
                .items[0]
                .id,
            tag.id
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(actor.credential_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(actor.credential_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET deleted_at=1 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE users SET deleted_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_tags_backend(&f.backend, f.workspace, &actor, None, 100)
                .await
                .unwrap(),
            Err(TagDbError::NotFound)
        ));
        f.close().await;
    }

    #[test]
    fn tag_pool_cleanup_failure_withholds_rows_and_retains_typed_causes() {
        // Pure propagation only; no actual remote cleanup is claimed.
        let pool = TagPool {
            can_create: true,
            can_manage: true,
            items: vec![],
        };
        let error = tag_pool_read_after_rollback(
            Ok(Ok(pool)),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        let unknown = source
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        assert!(unknown.original.is_none());
        assert!(matches!(&unknown.cleanup,sqlx::Error::Protocol(message) if message=="cleanup"));
        let error = tag_pool_read_after_rollback(
            Ok(Err(TagDbError::NotFound)),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        assert_eq!(
            source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap()
                .original
                .as_ref()
                .unwrap()
                .downcast_ref::<TagReadRefusal>()
                .unwrap()
                .0,
            TagDbError::NotFound
        );
        let error = tag_pool_read_after_rollback(
            Err(sqlx::Error::Protocol("original query".into())),
            Err(sqlx::Error::Protocol("cleanup".into())),
        )
        .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("missing cleanup envelope")
        };
        assert!(
            matches!(source.downcast_ref::<crate::db::backend::RollbackCleanupUnknown>().unwrap().original.as_ref().unwrap().downcast_ref::<sqlx::Error>(),Some(sqlx::Error::Protocol(message)) if message=="original query")
        );
    }
}
