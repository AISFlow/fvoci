//! Collections: typed fields and options, items, values and collection views.
//!
//! Source `packages/core/src/collection.ts` + `repos/collections.ts`. Every
//! operation runs in one transaction under the tenant context and re-checks
//! the live session, the workspace, the actor's membership and the current
//! project / wiki-document permission before touching rows:
//! - no read access to the collection (or target) reads as 404;
//! - read access without the needed level is 403 `insufficient_permissions`;
//! - an archived project rejects writes with 409 `project_archived`.

use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::collections::{
    AttachTarget, CalendarWindow, CollectionCreateInput, CollectionKind, CollectionValue,
    CollectionViewInput, DateBy, FieldCreateInput, FieldPatchInput, FieldType, GroupBy,
    QueryConfig, ValueInput, Visibility, FIELDS_PER_COLLECTION_MAX, WINDOW_DAYS_MAX,
};
use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{document_permission, membership_role, workspace_is_live};
use crate::db::identity::{append_audit, AuditAppend};
use crate::db::projects::{lock_project, project_permission, project_permission_by_id};
use crate::db::view_query::{compile_view_query, CompileOptions, RootKind, SqlArgs, ViewScope};
use crate::db::workspace::WorkspaceRole;
use crate::projects::{workspace_base_permission, ProjectPermission};
use crate::search::query::load_search_acl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionDbError {
    NotFound,
    Forbidden,
    InvalidInput,
    VersionConflict,
    ProjectArchived,
    TaskArchived,
    InvalidCursor,
}

pub type DbResult<T> = Result<Result<T, CollectionDbError>, sqlx::Error>;

macro_rules! bail {
    ($tx:expr, $err:expr) => {{
        $tx.rollback().await?;
        return Ok(Err($err));
    }};
}

macro_rules! check {
    ($tx:expr, $value:expr) => {
        match $value {
            Ok(value) => value,
            Err(err) => bail!($tx, err),
        }
    };
}

#[derive(Debug, Clone, Copy)]
pub struct Actor {
    pub user_id: Uuid,
    pub credential_id: Uuid,
    pub client_ip: Option<std::net::IpAddr>,
}

#[derive(Debug, Clone)]
pub struct CollectionRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub kind: CollectionKind,
    pub name: String,
    pub version: i32,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl CollectionRow {
    pub fn root_kind(&self) -> RootKind {
        match self.kind {
            CollectionKind::Document => RootKind::Document,
            CollectionKind::Task => RootKind::Task,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OptionRow {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub sort_key: String,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct FieldRow {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub field_type: FieldType,
    pub version: i32,
    pub sort_key: String,
    pub deleted_at: Option<DateTime<Utc>>,
    pub options: Vec<OptionRow>,
}

#[derive(Debug, Clone)]
pub struct ItemRow {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct CollectionViewRow {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub owner_id: Uuid,
    pub visibility: String,
    pub name: String,
    pub view_type: String,
    pub config: Value,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct ViewList {
    pub can_save: bool,
    pub can_manage: bool,
    pub items: Vec<CollectionViewRow>,
}

#[derive(Debug, Clone)]
pub struct ProjectCollection {
    pub collection: CollectionRow,
    pub can_edit: bool,
    pub can_manage: bool,
}

/// Effective level on a collection's scope and whether writes are blocked.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScopeAccess {
    pub permission: ProjectPermission,
    pub archived: bool,
}

/// Live session, live workspace, current membership. Writes serialise on the
/// actor's membership row so a concurrent removal/suspension cannot interleave.
pub(crate) async fn begin_member(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    write: bool,
) -> DbResult<WorkspaceRole> {
    set_tenant(tx, workspace_id).await?;
    let live = if write {
        lock_membership_users(tx, &[actor.user_id]).await?;
        recheck_session(tx, actor.user_id, actor.credential_id).await?
    } else {
        session_is_live(tx, actor.user_id, actor.credential_id).await?
    };
    if !live || !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(CollectionDbError::NotFound));
    }
    Ok(membership_role(tx, workspace_id, actor.user_id)
        .await?
        .ok_or(CollectionDbError::NotFound))
}

/// Project scope: current project permission (locked for writes so a
/// concurrent archive or member removal serialises); wiki scope: workspace base.
pub(crate) async fn scope_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
    project_id: Option<Uuid>,
    lock: bool,
) -> Result<Option<ScopeAccess>, sqlx::Error> {
    let Some(project_id) = project_id else {
        return Ok(Some(ScopeAccess {
            permission: workspace_base_permission(role),
            archived: false,
        }));
    };
    if lock {
        let Some(locked) = lock_project(tx, workspace_id, project_id).await? else {
            return Ok(None);
        };
        let permission = project_permission(tx, workspace_id, user_id, &locked).await?;
        return Ok(Some(ScopeAccess {
            permission,
            archived: locked.status == "archived",
        }));
    }
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(status) = status else {
        return Ok(None);
    };
    let permission = project_permission_by_id(tx, workspace_id, user_id, project_id)
        .await?
        .unwrap_or(ProjectPermission::None);
    Ok(Some(ScopeAccess {
        permission,
        archived: status == "archived",
    }))
}

type CollectionTuple = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    String,
    i32,
    Option<DateTime<Utc>>,
);

fn collection_from(row: CollectionTuple) -> CollectionRow {
    CollectionRow {
        id: row.0,
        workspace_id: row.1,
        project_id: row.2,
        kind: CollectionKind::parse(&row.3).unwrap_or(CollectionKind::Document),
        name: row.4,
        version: row.5,
        deleted_at: row.6,
    }
}

const COLLECTION_COLUMNS: &str = "id, workspace_id, project_id, kind, name, version, deleted_at";

async fn find_collection(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    collection_id: Uuid,
    lock: bool,
) -> Result<Option<CollectionRow>, sqlx::Error> {
    let sql = format!(
        "SELECT {COLLECTION_COLUMNS} FROM fvoci.collections WHERE workspace_id = $1 AND id = $2{}",
        if lock { " FOR UPDATE" } else { "" }
    );
    let row: Option<CollectionTuple> = sqlx::query_as(&sql)
        .bind(workspace_id)
        .bind(collection_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(row.map(collection_from))
}

/// A guest reads a wiki collection only through an item document it can read
/// (source `collections.readable`).
async fn guest_can_read_wiki_collection(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    collection_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let acl = load_search_acl(tx, workspace_id, user_id, WorkspaceRole::Guest, None).await?;
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.collection_items i
            JOIN fvoci.documents d ON d.workspace_id = i.workspace_id AND d.id = i.document_id
            WHERE i.workspace_id = $1 AND i.collection_id = $2 AND d.deleted_at IS NULL
              AND ((d.project_id IS NOT NULL AND d.project_id = ANY($3))
                   OR (d.project_id IS NULL AND ($4 OR d.id = ANY($5))))
        )
        "#,
    )
    .bind(workspace_id)
    .bind(collection_id)
    .bind(&acl.project_ids)
    .bind(acl.include_wiki)
    .bind(&acl.wiki_document_ids)
    .fetch_one(&mut **tx)
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    Read,
    Edit,
}

/// Source `requireCollection`: `Edit` locks the collection row first so field
/// configuration and value CAS serialise per collection.
pub(crate) async fn require_collection(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    role: WorkspaceRole,
    collection_id: Uuid,
    need: Need,
) -> DbResult<(CollectionRow, ScopeAccess)> {
    let Some(collection) =
        find_collection(tx, workspace_id, collection_id, need == Need::Edit).await?
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if collection.deleted_at.is_some() {
        return Ok(Err(CollectionDbError::NotFound));
    }
    let Some(access) = scope_access(
        tx,
        workspace_id,
        actor.user_id,
        role,
        collection.project_id,
        need == Need::Edit,
    )
    .await?
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    let readable = access.permission.at_least(ProjectPermission::View)
        || (collection.project_id.is_none()
            && role == WorkspaceRole::Guest
            && guest_can_read_wiki_collection(tx, workspace_id, actor.user_id, collection.id)
                .await?);
    if !readable {
        return Ok(Err(CollectionDbError::NotFound));
    }
    if need == Need::Edit {
        if !access.permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(CollectionDbError::Forbidden));
        }
        if access.archived {
            return Ok(Err(CollectionDbError::ProjectArchived));
        }
    }
    Ok(Ok((collection, access)))
}

async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    verb: &str,
    target_type: &str,
    target_id: Uuid,
    payload: Value,
) -> Result<(), sqlx::Error> {
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor.user_id),
            verb: verb.to_string(),
            target_type: Some(target_type.to_string()),
            target_id: Some(target_id),
            payload,
            ip: actor.client_ip.map(|ip| ip.to_string()),
        },
    )
    .await
}

pub async fn list_collections(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
) -> DbResult<Vec<CollectionRow>> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, false).await?);
    let acl = load_search_acl(&mut tx, workspace_id, actor.user_id, role, None).await?;
    let rows: Vec<CollectionTuple> = sqlx::query_as(&format!(
        "SELECT {COLLECTION_COLUMNS} FROM fvoci.collections \
         WHERE workspace_id = $1 AND deleted_at IS NULL ORDER BY name, id"
    ))
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = Vec::new();
    for row in rows {
        let collection = collection_from(row);
        let visible = match collection.project_id {
            Some(project_id) => acl.project_ids.contains(&project_id),
            None => {
                role != WorkspaceRole::Guest
                    || guest_can_read_wiki_collection(
                        &mut tx,
                        workspace_id,
                        actor.user_id,
                        collection.id,
                    )
                    .await?
            }
        };
        if visible {
            items.push(collection);
        }
    }
    tx.commit().await?;
    Ok(Ok(items))
}

pub async fn create_collection(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    input: &CollectionCreateInput,
) -> DbResult<CollectionRow> {
    // Task collections are created with their project.
    if input.kind == CollectionKind::Task {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    let Some(access) = scope_access(
        &mut tx,
        workspace_id,
        actor.user_id,
        role,
        input.project_id,
        true,
    )
    .await?
    else {
        bail!(tx, CollectionDbError::NotFound);
    };
    if !access.permission.at_least(ProjectPermission::View) {
        bail!(tx, CollectionDbError::NotFound);
    }
    if !access.permission.at_least(ProjectPermission::Edit) {
        bail!(tx, CollectionDbError::Forbidden);
    }
    if access.archived {
        bail!(tx, CollectionDbError::ProjectArchived);
    }
    let row: CollectionTuple = sqlx::query_as(&format!(
        "INSERT INTO fvoci.collections (id, workspace_id, project_id, kind, name) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLLECTION_COLUMNS}"
    ))
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(input.project_id)
    .bind(input.kind.as_str())
    .bind(&input.name)
    .fetch_one(&mut *tx)
    .await?;
    let collection = collection_from(row);
    audit(
        &mut tx,
        workspace_id,
        actor,
        "collection.created",
        "collection",
        collection.id,
        json!({"name": collection.name, "kind": collection.kind.as_str(), "projectId": collection.project_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(collection))
}

type OptionTuple = (Uuid, Uuid, String, String, String, Option<DateTime<Utc>>);

type FieldTuple = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    String,
    i32,
    String,
    Option<DateTime<Utc>>,
);

pub(crate) async fn load_fields(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    collection_id: Uuid,
) -> Result<Vec<FieldRow>, sqlx::Error> {
    let fields: Vec<FieldTuple> = sqlx::query_as(
        r#"
        SELECT id, collection_id, key, name, description, type, version, sort_key, deleted_at
        FROM fvoci.collection_fields
        WHERE workspace_id = $1 AND collection_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(collection_id)
    .fetch_all(&mut **tx)
    .await?;
    let options: Vec<OptionTuple> = sqlx::query_as(
        r#"
            SELECT field_id, id, key, label, sort_key, deleted_at
            FROM fvoci.collection_options
            WHERE workspace_id = $1 AND collection_id = $2
            ORDER BY sort_key COLLATE "C", id
            "#,
    )
    .bind(workspace_id)
    .bind(collection_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(fields
        .into_iter()
        .map(|f| FieldRow {
            id: f.0,
            collection_id: f.1,
            key: f.2,
            name: f.3,
            description: f.4,
            field_type: FieldType::parse(&f.5).unwrap_or(FieldType::Text),
            version: f.6,
            sort_key: f.7,
            deleted_at: f.8,
            options: options
                .iter()
                .filter(|o| o.0 == f.0)
                .map(|o| OptionRow {
                    id: o.1,
                    key: o.2.clone(),
                    label: o.3.clone(),
                    sort_key: o.4.clone(),
                    deleted_at: o.5,
                })
                .collect(),
        })
        .collect())
}

pub async fn list_fields(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
) -> DbResult<Vec<FieldRow>> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, false).await?);
    check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Read
        )
        .await?
    );
    let fields = load_fields(&mut tx, workspace_id, collection_id).await?;
    tx.commit().await?;
    Ok(Ok(fields))
}

fn pad_sort_key(index: usize) -> String {
    format!("{index:03}")
}

async fn insert_field(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    collection_id: Uuid,
    field: &FieldRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.collection_fields
            (id, workspace_id, collection_id, key, name, description, type, sort_key)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(field.id)
    .bind(workspace_id)
    .bind(collection_id)
    .bind(&field.key)
    .bind(&field.name)
    .bind(&field.description)
    .bind(field.field_type.as_str())
    .bind(&field.sort_key)
    .execute(&mut **tx)
    .await?;
    for option in &field.options {
        upsert_option(tx, workspace_id, collection_id, field.id, option).await?;
    }
    Ok(())
}

async fn upsert_option(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    collection_id: Uuid,
    field_id: Uuid,
    option: &OptionRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.collection_options
            (id, workspace_id, collection_id, field_id, key, label, sort_key, deleted_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (id) DO UPDATE
            SET label = EXCLUDED.label, sort_key = EXCLUDED.sort_key, deleted_at = EXCLUDED.deleted_at
        "#,
    )
    .bind(option.id)
    .bind(workspace_id)
    .bind(collection_id)
    .bind(field_id)
    .bind(&option.key)
    .bind(&option.label)
    .bind(&option.sort_key)
    .bind(option.deleted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn create_field(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    input: &FieldCreateInput,
) -> DbResult<FieldRow> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Edit
        )
        .await?
    );
    let fields = load_fields(&mut tx, workspace_id, collection_id).await?;
    if fields.len() >= FIELDS_PER_COLLECTION_MAX {
        bail!(tx, CollectionDbError::InvalidInput);
    }
    let key = input
        .key
        .clone()
        .unwrap_or_else(|| format!("f_{}", fields.len() + 1));
    if fields.iter().any(|field| field.key == key) {
        bail!(tx, CollectionDbError::InvalidInput);
    }
    let field = FieldRow {
        id: Uuid::now_v7(),
        collection_id,
        key,
        name: input.name.clone(),
        description: input.description.clone(),
        field_type: input.field_type,
        version: 1,
        sort_key: pad_sort_key(fields.len()),
        deleted_at: None,
        options: input
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| OptionRow {
                id: Uuid::now_v7(),
                key: format!("o_{}", index + 1),
                label: label.clone(),
                sort_key: pad_sort_key(index),
                deleted_at: None,
            })
            .collect(),
    };
    insert_field(&mut tx, workspace_id, collection_id, &field).await?;
    audit(
        &mut tx,
        workspace_id,
        actor,
        "collection_field.created",
        "collection_field",
        field.id,
        json!({"collectionId": collection_id, "key": field.key, "type": field.field_type.as_str()}),
    )
    .await?;
    let created = load_fields(&mut tx, workspace_id, collection_id)
        .await?
        .into_iter()
        .find(|row| row.id == field.id);
    tx.commit().await?;
    Ok(created.ok_or(CollectionDbError::NotFound))
}

pub async fn patch_field(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    field_id: Uuid,
    input: &FieldPatchInput,
) -> DbResult<FieldRow> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Edit
        )
        .await?
    );
    let fields = load_fields(&mut tx, workspace_id, collection_id).await?;
    let Some(field) = fields.into_iter().find(|field| field.id == field_id) else {
        bail!(tx, CollectionDbError::NotFound);
    };
    if field.version != input.expected_version {
        bail!(tx, CollectionDbError::VersionConflict);
    }
    let mut options = field.options.clone();
    if let Some(patches) = &input.options {
        if !field.field_type.has_options() {
            bail!(tx, CollectionDbError::InvalidInput);
        }
        // Every existing option must be listed by its stable id exactly once.
        let ids: Vec<Uuid> = patches.iter().filter_map(|patch| patch.id).collect();
        let unique: HashSet<Uuid> = ids.iter().copied().collect();
        if unique.len() != ids.len()
            || ids
                .iter()
                .any(|id| !field.options.iter().any(|option| option.id == *id))
            || field
                .options
                .iter()
                .any(|option| !unique.contains(&option.id))
        {
            bail!(tx, CollectionDbError::InvalidInput);
        }
        // Option keys are unique per field: new options continue after the
        // highest key ever used so a revived slot never collides.
        let mut last_key = field
            .options
            .iter()
            .filter_map(|option| option.key.strip_prefix("o_")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let now = Utc::now();
        options = patches
            .iter()
            .enumerate()
            .map(|(index, patch)| {
                let old = patch
                    .id
                    .and_then(|id| field.options.iter().find(|option| option.id == id));
                OptionRow {
                    id: patch.id.unwrap_or_else(Uuid::now_v7),
                    key: match old {
                        Some(old) => old.key.clone(),
                        None => {
                            last_key += 1;
                            format!("o_{last_key}")
                        }
                    },
                    label: patch.label.clone(),
                    sort_key: pad_sort_key(index),
                    deleted_at: if patch.deleted {
                        Some(old.and_then(|old| old.deleted_at).unwrap_or(now))
                    } else {
                        None
                    },
                }
            })
            .collect();
    }
    let deleted = input.deleted.unwrap_or(field.deleted_at.is_some());
    let description = match &input.description {
        None => field.description.clone(),
        Some(value) => value.clone(),
    };
    sqlx::query(
        r#"
        UPDATE fvoci.collection_fields
        SET name = $3,
            description = $4,
            deleted_at = CASE WHEN $5 THEN COALESCE(deleted_at, now()) ELSE NULL END,
            version = version + 1,
            updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(field_id)
    .bind(input.name.as_deref().unwrap_or(&field.name))
    .bind(&description)
    .bind(deleted)
    .execute(&mut *tx)
    .await?;
    if input.options.is_some() {
        for option in &options {
            upsert_option(&mut tx, workspace_id, collection_id, field_id, option).await?;
        }
    }
    audit(
        &mut tx,
        workspace_id,
        actor,
        "collection_field.updated",
        "collection_field",
        field_id,
        json!({"collectionId": collection_id, "deleted": deleted, "version": field.version + 1}),
    )
    .await?;
    let updated = load_fields(&mut tx, workspace_id, collection_id)
        .await?
        .into_iter()
        .find(|row| row.id == field_id);
    tx.commit().await?;
    Ok(updated.ok_or(CollectionDbError::NotFound))
}

/// Resolved attach/value target: its project and the actor's level on it.
pub(crate) struct TargetInfo {
    pub project_id: Option<Uuid>,
    /// The actor could write this target now (level, project and task live).
    pub can_edit: bool,
}

/// Source `requirePermission(target, need)` plus the root/archival checks.
pub(crate) async fn require_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: &Actor,
    role: WorkspaceRole,
    target: AttachTarget,
    need: Need,
) -> DbResult<TargetInfo> {
    let (project_id, task_archived) = match target {
        AttachTarget::Document(id) => {
            let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
                "SELECT project_id, deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
            match row {
                Some((project_id, None)) => (project_id, false),
                _ => return Ok(Err(CollectionDbError::NotFound)),
            }
        }
        AttachTarget::Task(id) => {
            let row: Option<TaskScopeTuple> =
                sqlx::query_as(
                    "SELECT project_id, deleted_at, archived_at FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
                )
                .bind(workspace_id)
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
            match row {
                Some((project_id, None, archived_at)) => (Some(project_id), archived_at.is_some()),
                _ => return Ok(Err(CollectionDbError::NotFound)),
            }
        }
    };
    let (permission, archived) = match (target, project_id) {
        (AttachTarget::Document(id), None) => (
            document_permission(tx, workspace_id, actor.user_id, id, true).await?,
            false,
        ),
        (_, project_id) => match scope_access(
            tx,
            workspace_id,
            actor.user_id,
            role,
            project_id,
            need == Need::Edit,
        )
        .await?
        {
            Some(access) => (access.permission, access.archived),
            None => return Ok(Err(CollectionDbError::NotFound)),
        },
    };
    if !permission.at_least(ProjectPermission::View) {
        return Ok(Err(CollectionDbError::NotFound));
    }
    if need == Need::Edit {
        if !permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(CollectionDbError::Forbidden));
        }
        if archived {
            return Ok(Err(CollectionDbError::ProjectArchived));
        }
        if task_archived {
            return Ok(Err(CollectionDbError::TaskArchived));
        }
    }
    Ok(Ok(TargetInfo {
        project_id,
        can_edit: permission.at_least(ProjectPermission::Edit) && !archived && !task_archived,
    }))
}

type TaskScopeTuple = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

type ScalarColumns = (
    Option<String>,
    Option<String>,
    Option<NaiveDate>,
    Option<DateTime<Utc>>,
    Option<bool>,
);

type ItemTuple = (Uuid, Uuid, Option<Uuid>, Option<Uuid>, i32);

fn item_from(row: ItemTuple) -> ItemRow {
    ItemRow {
        id: row.0,
        collection_id: row.1,
        document_id: row.2,
        task_id: row.3,
        version: row.4,
    }
}

async fn find_item_by_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: AttachTarget,
) -> Result<Option<ItemRow>, sqlx::Error> {
    let (document_id, task_id) = match target {
        AttachTarget::Document(id) => (Some(id), None),
        AttachTarget::Task(id) => (None, Some(id)),
    };
    let row: Option<ItemTuple> = sqlx::query_as(
        r#"
        SELECT id, collection_id, document_id, task_id, version
        FROM fvoci.collection_items
        WHERE workspace_id = $1
          AND (($2::uuid IS NOT NULL AND document_id = $2) OR ($3::uuid IS NOT NULL AND task_id = $3))
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(item_from))
}

pub async fn attach_item(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    target: AttachTarget,
) -> DbResult<ItemRow> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    let (collection, _) = check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Edit
        )
        .await?
    );
    let kind_matches = matches!(
        (collection.kind, target),
        (CollectionKind::Document, AttachTarget::Document(_))
            | (CollectionKind::Task, AttachTarget::Task(_))
    );
    if !kind_matches {
        bail!(tx, CollectionDbError::InvalidInput);
    }
    let info = check!(
        tx,
        require_target(&mut tx, workspace_id, actor, role, target, Need::Edit).await?
    );
    if info.project_id != collection.project_id {
        bail!(tx, CollectionDbError::InvalidInput);
    }
    if let Some(existing) = find_item_by_target(&mut tx, workspace_id, target).await? {
        if existing.collection_id != collection_id {
            bail!(tx, CollectionDbError::InvalidInput);
        }
        tx.commit().await?;
        return Ok(Ok(existing));
    }
    let (document_id, task_id) = match target {
        AttachTarget::Document(id) => (Some(id), None),
        AttachTarget::Task(id) => (None, Some(id)),
    };
    let inserted: Option<ItemTuple> = sqlx::query_as(
        r#"
        INSERT INTO fvoci.collection_items (id, workspace_id, collection_id, document_id, task_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        RETURNING id, collection_id, document_id, task_id, version
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(collection_id)
    .bind(document_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let item = match inserted {
        Some(row) => item_from(row),
        // A concurrent attach won the unique slot.
        None => match find_item_by_target(&mut tx, workspace_id, target).await? {
            Some(existing) if existing.collection_id == collection_id => existing,
            _ => bail!(tx, CollectionDbError::InvalidInput),
        },
    };
    tx.commit().await?;
    Ok(Ok(item))
}

pub async fn put_value(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    item_id: Uuid,
    input: &ValueInput,
) -> DbResult<i32> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    // Source: lock the collection, then require read on it; the edit
    // requirement is on the item's document/task.
    if find_collection(&mut tx, workspace_id, collection_id, true)
        .await?
        .is_none()
    {
        bail!(tx, CollectionDbError::NotFound);
    }
    let (_, access) = check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Read
        )
        .await?
    );
    if access.archived {
        bail!(tx, CollectionDbError::ProjectArchived);
    }
    let item: Option<ItemTuple> = sqlx::query_as(
        r#"
        SELECT id, collection_id, document_id, task_id, version
        FROM fvoci.collection_items WHERE workspace_id = $1 AND id = $2 FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(item_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(item) = item
        .map(item_from)
        .filter(|item| item.collection_id == collection_id)
    else {
        bail!(tx, CollectionDbError::NotFound);
    };
    let target = match (item.document_id, item.task_id) {
        (Some(id), _) => AttachTarget::Document(id),
        (_, Some(id)) => AttachTarget::Task(id),
        _ => bail!(tx, CollectionDbError::NotFound),
    };
    check!(
        tx,
        require_target(&mut tx, workspace_id, actor, role, target, Need::Edit).await?
    );
    if item.version != input.expected_version {
        bail!(tx, CollectionDbError::VersionConflict);
    }
    let fields = load_fields(&mut tx, workspace_id, collection_id).await?;
    let Some(field) = fields
        .into_iter()
        .find(|field| field.id == input.field_id && field.deleted_at.is_none())
    else {
        bail!(tx, CollectionDbError::NotFound);
    };
    if field.version != input.expected_field_version {
        bail!(tx, CollectionDbError::VersionConflict);
    }
    check!(
        tx,
        validate_value(&mut tx, workspace_id, item.id, &field, &input.value).await?
    );
    write_value(&mut tx, workspace_id, &item, &field, &input.value).await?;
    tx.commit().await?;
    Ok(Ok(item.version + 1))
}

async fn validate_value(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    item_id: Uuid,
    field: &FieldRow,
    value: &CollectionValue,
) -> DbResult<()> {
    if !value.fits(field.field_type) {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    match value {
        CollectionValue::Options(ids) => {
            let unique: HashSet<Uuid> = ids.iter().copied().collect();
            if unique.len() != ids.len() || (field.field_type == FieldType::Select && ids.len() > 1)
            {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
            // An archived option may stay selected but cannot be newly chosen.
            let previous: Vec<Uuid> = sqlx::query_scalar(
                "SELECT option_id FROM fvoci.collection_choices WHERE workspace_id = $1 AND item_id = $2 AND field_id = $3",
            )
            .bind(workspace_id)
            .bind(item_id)
            .bind(field.id)
            .fetch_all(&mut **tx)
            .await?;
            for id in ids {
                let Some(option) = field.options.iter().find(|option| option.id == *id) else {
                    return Ok(Err(CollectionDbError::InvalidInput));
                };
                if option.deleted_at.is_some() && !previous.contains(id) {
                    return Ok(Err(CollectionDbError::InvalidInput));
                }
            }
        }
        CollectionValue::Users(ids) => {
            let unique: HashSet<Uuid> = ids.iter().copied().collect();
            if unique.len() != ids.len() || (field.field_type == FieldType::User && ids.len() > 1) {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
            let members: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = ANY($2)",
            )
            .bind(workspace_id)
            .bind(ids)
            .fetch_one(&mut **tx)
            .await?;
            if members as usize != ids.len() {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

async fn write_value(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    item: &ItemRow,
    field: &FieldRow,
    value: &CollectionValue,
) -> Result<(), sqlx::Error> {
    for table in [
        "collection_values",
        "collection_choices",
        "collection_people",
    ] {
        sqlx::query(&format!(
            "DELETE FROM fvoci.{table} WHERE workspace_id = $1 AND item_id = $2 AND field_id = $3"
        ))
        .bind(workspace_id)
        .bind(item.id)
        .bind(field.id)
        .execute(&mut **tx)
        .await?;
    }
    let field_type = field.field_type.as_str();
    match value {
        CollectionValue::Null => {}
        CollectionValue::Options(ids) => {
            sqlx::query(
                r#"
                INSERT INTO fvoci.collection_choices
                    (workspace_id, collection_id, item_id, field_id, field_type, option_id)
                SELECT $1, $2, $3, $4, $5, option_id FROM unnest($6::uuid[]) AS option_id
                "#,
            )
            .bind(workspace_id)
            .bind(item.collection_id)
            .bind(item.id)
            .bind(field.id)
            .bind(field_type)
            .bind(ids)
            .execute(&mut **tx)
            .await?;
        }
        CollectionValue::Users(ids) => {
            sqlx::query(
                r#"
                INSERT INTO fvoci.collection_people
                    (workspace_id, collection_id, item_id, field_id, field_type, user_id)
                SELECT $1, $2, $3, $4, $5, user_id FROM unnest($6::uuid[]) AS user_id
                "#,
            )
            .bind(workspace_id)
            .bind(item.collection_id)
            .bind(item.id)
            .bind(field.id)
            .bind(field_type)
            .bind(ids)
            .execute(&mut **tx)
            .await?;
        }
        scalar => {
            let (text, number, date, ts, flag): ScalarColumns = match scalar {
                CollectionValue::Text(text) => (Some(text.clone()), None, None, None, None),
                CollectionValue::Number(number) => (
                    None,
                    Some(crate::db::view_query::format_number(*number)),
                    None,
                    None,
                    None,
                ),
                CollectionValue::Date(date) => (None, None, Some(*date), None, None),
                CollectionValue::Datetime(at) => (None, None, None, Some(*at), None),
                CollectionValue::Checkbox(flag) => (None, None, None, None, Some(*flag)),
                _ => (None, None, None, None, None),
            };
            sqlx::query(
                r#"
                INSERT INTO fvoci.collection_values
                    (workspace_id, collection_id, item_id, field_id, field_type,
                     value_text, value_number, value_date, value_ts, value_bool)
                VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8, $9, $10)
                "#,
            )
            .bind(workspace_id)
            .bind(item.collection_id)
            .bind(item.id)
            .bind(field.id)
            .bind(field_type)
            .bind(text)
            .bind(number)
            .bind(date)
            .bind(ts)
            .bind(flag)
            .execute(&mut **tx)
            .await?;
        }
    }
    sqlx::query(
        "UPDATE fvoci.collection_items SET version = version + 1, updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(item.id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Source `validateQuery`: grouping/date bases must fit the collection kind
/// and reference live fields of the right type; calendar windows are bounded.
pub(crate) async fn validate_query_config(
    tx: &mut Transaction<'_, Postgres>,
    collection: &CollectionRow,
    fields: &[FieldRow],
    config: &QueryConfig,
    day_given: bool,
    window: Option<&CalendarWindow>,
) -> Result<Result<(), CollectionDbError>, sqlx::Error> {
    let live = |id: Uuid| {
        fields
            .iter()
            .find(|field| field.id == id && field.deleted_at.is_none())
    };
    match config.group_by {
        Some(GroupBy::Status) if collection.kind != CollectionKind::Task => {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        Some(GroupBy::Field(id)) => match live(id) {
            Some(field) if field.field_type == FieldType::Select => {}
            _ => return Ok(Err(CollectionDbError::InvalidInput)),
        },
        _ => {}
    }
    match config.date_by {
        Some(DateBy::Due | DateBy::Start) if collection.kind != CollectionKind::Task => {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        Some(DateBy::Field(id)) => match live(id) {
            Some(field) if matches!(field.field_type, FieldType::Date | FieldType::Datetime) => {}
            _ => return Ok(Err(CollectionDbError::InvalidInput)),
        },
        _ => {}
    }
    if day_given && window.is_none() {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    if let Some(window) = window {
        if config.date_by.is_none()
            || window.from >= window.to
            || (window.to - window.from).num_days() > WINDOW_DAYS_MAX
        {
            return Ok(Err(CollectionDbError::InvalidInput));
        }
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name = $1)",
        )
        .bind(&window.time_zone)
        .fetch_one(&mut **tx)
        .await?;
        if !known {
            return Ok(Err(CollectionDbError::InvalidInput));
        }
    }
    Ok(Ok(()))
}

type ViewTuple = (Uuid, Uuid, Uuid, String, String, String, Value, i32);

fn view_from(row: ViewTuple) -> CollectionViewRow {
    CollectionViewRow {
        id: row.0,
        collection_id: row.1,
        owner_id: row.2,
        visibility: row.3,
        name: row.4,
        view_type: row.5,
        config: row.6,
        version: row.7,
    }
}

const VIEW_COLUMNS: &str = "id, collection_id, owner_id, visibility, name, type, config, version";

async fn visible_views(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<CollectionViewRow>, sqlx::Error> {
    let rows: Vec<ViewTuple> = sqlx::query_as(&format!(
        "SELECT {VIEW_COLUMNS} FROM fvoci.collection_views \
         WHERE workspace_id = $1 AND collection_id = $2 AND (owner_id = $3 OR visibility = 'shared') \
         ORDER BY name, id"
    ))
    .bind(workspace_id)
    .bind(collection_id)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(view_from).collect())
}

pub async fn list_views(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
) -> DbResult<ViewList> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, false).await?);
    let (_, access) = check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Read
        )
        .await?
    );
    let items = visible_views(&mut tx, workspace_id, collection_id, actor.user_id).await?;
    tx.commit().await?;
    let can_save = !access.archived;
    Ok(Ok(ViewList {
        can_save,
        can_manage: can_save && access.permission == ProjectPermission::Manage,
        items,
    }))
}

/// Row lock on a collection view before its owner/visibility checks, so a
/// concurrent visibility change cannot slip between the check and the write.
async fn lock_view(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    view_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT 1 FROM fvoci.collection_views WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(view_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(())
}

/// Create (`view_id` None) or compare-and-swap update a collection view.
pub async fn save_view(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    view_id: Option<Uuid>,
    input: &CollectionViewInput,
) -> DbResult<CollectionViewRow> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    let (collection, access) = check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Read
        )
        .await?
    );
    if access.archived {
        bail!(tx, CollectionDbError::ProjectArchived);
    }
    let fields = load_fields(&mut tx, workspace_id, collection_id).await?;
    check!(
        tx,
        validate_query_config(&mut tx, &collection, &fields, &input.config, false, None).await?
    );
    let mut args = SqlArgs::starting_at(1);
    let compiled = compile_view_query(
        &mut tx,
        ViewScope {
            workspace_id,
            project_id: collection.project_id,
            collection_id: Some(collection_id),
            kind: collection.root_kind(),
        },
        &input.config.query,
        &CompileOptions {
            actor_user_id: actor.user_id,
            time_zone: "UTC",
            standard_filters: true,
        },
        "r",
        &mut args,
    )
    .await?;
    if compiled.is_err() {
        bail!(tx, CollectionDbError::InvalidInput);
    }
    let manage = access.permission == ProjectPermission::Manage;
    if input.visibility == Visibility::Shared && !manage {
        bail!(tx, CollectionDbError::Forbidden);
    }
    let config = input.config.to_json();
    let row: Option<ViewTuple> = match view_id {
        None => Some(
            sqlx::query_as(&format!(
                "INSERT INTO fvoci.collection_views \
                 (id, workspace_id, collection_id, owner_id, visibility, name, type, config) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {VIEW_COLUMNS}"
            ))
            .bind(Uuid::now_v7())
            .bind(workspace_id)
            .bind(collection_id)
            .bind(actor.user_id)
            .bind(input.visibility.as_str())
            .bind(&input.name)
            .bind(input.view_type.as_str())
            .bind(&config)
            .fetch_one(&mut *tx)
            .await?,
        ),
        Some(view_id) => {
            // Lock first so owner/visibility checks see the row the write changes.
            lock_view(&mut tx, workspace_id, view_id).await?;
            let views = visible_views(&mut tx, workspace_id, collection_id, actor.user_id).await?;
            let Some(old) = views.into_iter().find(|view| view.id == view_id) else {
                bail!(tx, CollectionDbError::NotFound);
            };
            if old.visibility != input.visibility.as_str() && old.owner_id != actor.user_id {
                bail!(tx, CollectionDbError::InvalidInput);
            }
            if old.visibility == "shared" {
                if !manage {
                    bail!(tx, CollectionDbError::Forbidden);
                }
            } else if old.owner_id != actor.user_id {
                bail!(tx, CollectionDbError::NotFound);
            }
            let Some(expected) = input.expected_version else {
                bail!(tx, CollectionDbError::InvalidInput);
            };
            sqlx::query_as(&format!(
                "UPDATE fvoci.collection_views \
                 SET name = $4, type = $5, visibility = $6, config = $7, version = version + 1, updated_at = now() \
                 WHERE workspace_id = $1 AND id = $2 AND version = $3 RETURNING {VIEW_COLUMNS}"
            ))
            .bind(workspace_id)
            .bind(view_id)
            .bind(expected)
            .bind(&input.name)
            .bind(input.view_type.as_str())
            .bind(input.visibility.as_str())
            .bind(&config)
            .fetch_optional(&mut *tx)
            .await?
        }
    };
    let Some(row) = row else {
        bail!(tx, CollectionDbError::VersionConflict);
    };
    let view = view_from(row);
    if view.visibility == "shared" {
        audit(
            &mut tx,
            workspace_id,
            actor,
            if view_id.is_some() {
                "collection_view.updated"
            } else {
                "collection_view.created"
            },
            "collection_view",
            view.id,
            json!({"collectionId": collection_id, "name": view.name, "version": view.version}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(view))
}

pub async fn remove_view(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    view_id: Uuid,
) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, true).await?);
    let (_, access) = check!(
        tx,
        require_collection(
            &mut tx,
            workspace_id,
            actor,
            role,
            collection_id,
            Need::Read
        )
        .await?
    );
    if access.archived {
        bail!(tx, CollectionDbError::ProjectArchived);
    }
    // Lock first so owner/visibility checks see the row the delete removes.
    lock_view(&mut tx, workspace_id, view_id).await?;
    let views = visible_views(&mut tx, workspace_id, collection_id, actor.user_id).await?;
    let Some(old) = views.into_iter().find(|view| view.id == view_id) else {
        bail!(tx, CollectionDbError::NotFound);
    };
    if old.visibility == "shared" {
        if access.permission != ProjectPermission::Manage {
            bail!(tx, CollectionDbError::Forbidden);
        }
    } else if old.owner_id != actor.user_id {
        bail!(tx, CollectionDbError::NotFound);
    }
    let deleted = sqlx::query(
        "DELETE FROM fvoci.collection_views WHERE workspace_id = $1 AND collection_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(collection_id)
    .bind(view_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        bail!(tx, CollectionDbError::NotFound);
    }
    if old.visibility == "shared" {
        audit(
            &mut tx,
            workspace_id,
            actor,
            "collection_view.deleted",
            "collection_view",
            view_id,
            json!({"collectionId": collection_id, "name": old.name}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(()))
}

#[derive(Debug, Clone)]
pub struct ItemLookup {
    pub item: Option<ItemRow>,
    /// Field id → value JSON of the item (empty when not in a collection).
    pub values: Vec<(Uuid, Value)>,
    pub can_edit: bool,
}

/// Source `getCollectionItem`: `item: None` when the readable target is not in
/// any collection. Also returns the item's values and whether the actor may
/// write them, so a detail page needs no collection query (Rust addition).
pub async fn item_for_target(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    target: AttachTarget,
) -> DbResult<ItemLookup> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, false).await?);
    let info = check!(
        tx,
        require_target(&mut tx, workspace_id, actor, role, target, Need::Read).await?
    );
    let item = find_item_by_target(&mut tx, workspace_id, target).await?;
    let values = match &item {
        Some(item) => {
            crate::db::collection_query::load_item_values(&mut tx, workspace_id, &[item.id])
                .await?
                .into_iter()
                .map(|(_, field_id, value)| (field_id, value))
                .collect()
        }
        None => Vec::new(),
    };
    tx.commit().await?;
    Ok(Ok(ItemLookup {
        item,
        values,
        can_edit: info.can_edit,
    }))
}

pub async fn project_collection(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
) -> DbResult<ProjectCollection> {
    let mut tx = pool.begin().await?;
    let role = check!(tx, begin_member(&mut tx, workspace_id, actor, false).await?);
    let Some(access) = scope_access(
        &mut tx,
        workspace_id,
        actor.user_id,
        role,
        Some(project_id),
        false,
    )
    .await?
    else {
        bail!(tx, CollectionDbError::NotFound);
    };
    if !access.permission.at_least(ProjectPermission::View) {
        bail!(tx, CollectionDbError::NotFound);
    }
    let row: Option<CollectionTuple> = sqlx::query_as(&format!(
        "SELECT {COLLECTION_COLUMNS} FROM fvoci.collections \
         WHERE workspace_id = $1 AND project_id = $2 AND kind = 'task'"
    ))
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        bail!(tx, CollectionDbError::NotFound);
    };
    tx.commit().await?;
    Ok(Ok(ProjectCollection {
        collection: collection_from(row),
        can_edit: access.permission.at_least(ProjectPermission::Edit) && !access.archived,
        can_manage: access.permission == ProjectPermission::Manage,
    }))
}

/// Source `transferSharedViewOwnership`: a removed member's shared views stay
/// with the collection (the remover takes ownership); private views cascade.
pub(crate) async fn transfer_shared_view_ownership(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    from_user_id: Uuid,
    to_user_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE fvoci.collection_views
        SET owner_id = $3, version = version + 1, updated_at = now()
        WHERE workspace_id = $1 AND owner_id = $2 AND visibility = 'shared'
        "#,
    )
    .bind(workspace_id)
    .bind(from_user_id)
    .bind(to_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

/// Project clone: copy the source task collection's live fields/options and
/// its shared views (remapped by `remap`) into the destination collection.
pub(crate) async fn copy_task_collection(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_project_id: Uuid,
    dest_project_id: Uuid,
    owner_id: Uuid,
    remap: &mut crate::db::project_clone::CloneCatalog,
) -> Result<Result<(), CollectionDbError>, sqlx::Error> {
    let ids: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT s.id, d.id
        FROM fvoci.collections s, fvoci.collections d
        WHERE s.workspace_id = $1 AND s.project_id = $2 AND s.kind = 'task'
          AND d.workspace_id = $1 AND d.project_id = $3 AND d.kind = 'task'
        "#,
    )
    .bind(workspace_id)
    .bind(source_project_id)
    .bind(dest_project_id)
    .fetch_all(&mut **tx)
    .await?;
    let Some((source_id, dest_id)) = ids.first().copied() else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    for field in load_fields(tx, workspace_id, source_id).await? {
        if field.deleted_at.is_some() {
            continue;
        }
        let new_id = Uuid::now_v7();
        remap.fields.insert(field.id, new_id);
        let mut option_map = matches!(
            field.field_type,
            FieldType::Select | FieldType::MultiSelect | FieldType::Checkboxes
        )
        .then(std::collections::HashMap::new);
        let mut options = Vec::new();
        for option in field.options.iter().filter(|o| o.deleted_at.is_none()) {
            let option_id = Uuid::now_v7();
            if let Some(map) = option_map.as_mut() {
                map.insert(option.id, option_id);
            }
            options.push(OptionRow {
                id: option_id,
                ..option.clone()
            });
        }
        if let Some(map) = option_map {
            remap.options_by_field.insert(field.id, map);
        }
        let copy = FieldRow {
            id: new_id,
            collection_id: dest_id,
            options,
            ..field
        };
        insert_field(tx, workspace_id, dest_id, &copy).await?;
    }
    let views: Vec<ViewTuple> = sqlx::query_as(&format!(
        "SELECT {VIEW_COLUMNS} FROM fvoci.collection_views \
         WHERE workspace_id = $1 AND collection_id = $2 AND visibility = 'shared' ORDER BY name, id"
    ))
    .bind(workspace_id)
    .bind(source_id)
    .fetch_all(&mut **tx)
    .await?;
    for view in views.into_iter().map(view_from) {
        let Some(config) = remap.remap_collection_config(&view.config) else {
            return Ok(Err(CollectionDbError::InvalidInput));
        };
        sqlx::query(
            r#"
            INSERT INTO fvoci.collection_views
                (id, workspace_id, collection_id, owner_id, visibility, name, type, config)
            VALUES ($1, $2, $3, $4, 'shared', $5, $6, $7)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(dest_id)
        .bind(owner_id)
        .bind(&view.name)
        .bind(&view.view_type)
        .bind(&config)
        .execute(&mut **tx)
        .await?;
    }
    Ok(Ok(()))
}
