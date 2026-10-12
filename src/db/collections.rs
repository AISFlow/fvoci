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
use crate::db::documents::document_permission;
use crate::db::identity::{append_audit, AuditAppend};
use crate::db::projects::{lock_project, project_permission, project_permission_by_id};
use crate::db::view_query::{compile_view_query, CompileOptions, RootKind, SqlArgs, ViewScope};
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::{workspace_base_permission, ProjectPermission};
use crate::search::query::{load_search_acl, SearchAcl};

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
/// (source `collections.readable`); `acl` is the guest's workspace ACL.
async fn guest_can_read_wiki_collection(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    acl: &SearchAcl,
    collection_id: Uuid,
) -> Result<bool, sqlx::Error> {
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
    let mut readable = access.permission.at_least(ProjectPermission::View);
    if !readable && collection.project_id.is_none() && role == WorkspaceRole::Guest {
        let acl = load_search_acl(tx, workspace_id, actor.user_id, role, None).await?;
        readable = guest_can_read_wiki_collection(tx, workspace_id, &acl, collection.id).await?;
    }
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
                    || guest_can_read_wiki_collection(&mut tx, workspace_id, &acl, collection.id)
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

pub async fn list_collections_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
) -> DbResult<Vec<CollectionRow>> {
    if let Backend::Postgres(pool) = backend {
        return list_collections(pool, workspace_id, actor).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_list_collections(family, workspace_id, actor).await;
    finish_collection(tx, result, false).await
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

pub async fn create_collection_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    input: &CollectionCreateInput,
) -> DbResult<CollectionRow> {
    if let Backend::Postgres(pool) = backend {
        return create_collection(pool, workspace_id, actor, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_create_collection(family, workspace_id, actor, input).await;
    finish_collection(tx, result, true).await
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

pub async fn list_fields_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
) -> DbResult<Vec<FieldRow>> {
    if let Backend::Postgres(pool) = backend {
        return list_fields(pool, workspace_id, actor, collection_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_list_fields(family, workspace_id, actor, collection_id).await;
    finish_collection(tx, result, false).await
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

pub async fn create_field_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    input: &FieldCreateInput,
) -> DbResult<FieldRow> {
    if let Backend::Postgres(pool) = backend {
        return create_field(pool, workspace_id, actor, collection_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_create_field(family, workspace_id, actor, collection_id, input).await;
    finish_collection(tx, result, true).await
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

pub async fn patch_field_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    field_id: Uuid,
    input: &FieldPatchInput,
) -> DbResult<FieldRow> {
    if let Backend::Postgres(pool) = backend {
        return patch_field(pool, workspace_id, actor, collection_id, field_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result =
        family_patch_field(family, workspace_id, actor, collection_id, field_id, input).await;
    finish_collection(tx, result, true).await
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

pub async fn attach_item_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    target: AttachTarget,
) -> DbResult<ItemRow> {
    if let Backend::Postgres(pool) = backend {
        return attach_item(pool, workspace_id, actor, collection_id, target).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_attach_item(family, workspace_id, actor, collection_id, target).await;
    finish_collection(tx, result, true).await
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

pub async fn put_value_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    item_id: Uuid,
    input: &ValueInput,
) -> DbResult<i32> {
    if let Backend::Postgres(pool) = backend {
        return put_value(pool, workspace_id, actor, collection_id, item_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_put_value(family, workspace_id, actor, collection_id, item_id, input).await;
    finish_collection(tx, result, true).await
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

pub async fn list_views_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
) -> DbResult<ViewList> {
    if let Backend::Postgres(pool) = backend {
        return list_views(pool, workspace_id, actor, collection_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_list_views(family, workspace_id, actor, collection_id).await;
    finish_collection(tx, result, false).await
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

pub async fn save_view_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    view_id: Option<Uuid>,
    input: &CollectionViewInput,
) -> DbResult<CollectionViewRow> {
    if let Backend::Postgres(pool) = backend {
        return save_view(pool, workspace_id, actor, collection_id, view_id, input).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_save_view(family, workspace_id, actor, collection_id, view_id, input).await;
    finish_collection(tx, result, true).await
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

pub async fn remove_view_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    view_id: Uuid,
) -> DbResult<()> {
    if let Backend::Postgres(pool) = backend {
        return remove_view(pool, workspace_id, actor, collection_id, view_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_remove_view(family, workspace_id, actor, collection_id, view_id).await;
    finish_collection(tx, result, true).await
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

pub async fn item_for_target_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    target: AttachTarget,
) -> DbResult<ItemLookup> {
    if let Backend::Postgres(pool) = backend {
        return item_for_target(pool, workspace_id, actor, target).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_item_for_target(family, workspace_id, actor, target).await;
    finish_collection(tx, result, false).await
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

pub async fn project_collection_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: &Actor,
    project_id: Uuid,
) -> DbResult<ProjectCollection> {
    if let Backend::Postgres(pool) = backend {
        return project_collection(pool, workspace_id, actor, project_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        unreachable!()
    };
    let result = family_project_collection(family, workspace_id, actor, project_id).await;
    finish_collection(tx, result, false).await
}

/// Source `transferSharedViewOwnership`: a removed member's shared views stay
/// with the collection (the remover takes ownership); private views cascade.
impl crate::db::backend::OperationTx<'_, '_> {
    pub(crate) async fn transfer_shared_view_ownership(
        &mut self,
        workspace: Uuid,
        from: Uuid,
        to: Uuid,
    ) -> Result<u64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => transfer_shared_view_ownership(tx, workspace, from, to).await,
            Self::SqliteFamily(tx) => {
                use crate::db::codec::Cell;
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute(
                    "UPDATE collection_views SET owner_id=?3,version=version+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND owner_id=?2 AND visibility='shared'",
                    &[Cell::uuid(workspace), Cell::uuid(from), Cell::uuid(to)],
                ).await
            }
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_member_removal_view_tests {
    use super::*;
    use crate::db::workspace::selected_member_removal_tests::{fixture, remove, snapshot, views};

    #[tokio::test]
    async fn sqlite_shared_view_transfer_scope_version_overflow_rollback_and_healthy_progress() {
        let (f, credential, target, _) = fixture().await;
        let (shared, private) = views(&f, target).await;
        let before = snapshot(&f).await;
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_tenant(f.workspace).await.unwrap();
        assert!(read
            .operation()
            .transfer_shared_view_ownership(f.workspace, target, f.user)
            .await
            .is_err());
        read.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        {
            let mut op = tx.operation();
            op.set_tenant(f.workspace).await.unwrap();
            assert!(op
                .transfer_shared_view_ownership(Uuid::now_v7(), target, f.user)
                .await
                .is_err());
            assert_eq!(
                op.transfer_shared_view_ownership(f.workspace, target, f.user)
                    .await
                    .unwrap(),
                1
            );
        }
        tx.rollback().await.unwrap();
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE collection_views SET version=2147483647 WHERE id=?1")
            .bind(shared.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(matches!(
            remove(&f, credential, target).await,
            Err(sqlx::Error::Database(_))
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE collection_views SET version=7 WHERE id=?1")
            .bind(shared.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        remove(&f, credential, target).await.unwrap().unwrap();
        let stored: (Vec<u8>, String, String, i64, i64) = sqlx::query_as(
            "SELECT owner_id,name,config,version,updated_at FROM collection_views WHERE id=?1",
        )
        .bind(shared.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(stored.0, f.user.as_bytes());
        assert_eq!(stored.1, "Literal retained view");
        assert_eq!(stored.2, "{\"filter\":\"unchanged\"}");
        assert_eq!(stored.3, 8);
        assert!(stored.4 > 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM collection_views WHERE id=?1")
                .bind(private.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        crate::db::workspace::selected_personal_workspace_tests::foreign_keys(&f).await;
        f.close().await;
    }
}

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
        // Source OPTION_FIELD_TYPES: select, multi_select, checkboxes, labels.
        let mut option_map = field
            .field_type
            .has_options()
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

use crate::db::backend::{Backend, DbTransaction, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

macro_rules! family_check {
    ($value:expr) => {
        match $value {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        }
    };
}

pub(crate) async fn finish_collection<T>(
    tx: DbTx,
    result: DbResult<T>,
    write: bool,
) -> DbResult<T> {
    if write && matches!(result, Ok(Ok(_))) {
        tx.commit_with_cleanup()
            .await
            .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    } else if let Err(cleanup) = tx.rollback().await {
        let original = match result {
            Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                "collection refusal: {refusal:?}"
            ))) as Box<dyn std::error::Error + Send + Sync>),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}

pub(crate) async fn family_begin_member(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    write: bool,
) -> DbResult<WorkspaceRole> {
    let mut op = OperationTx::SqliteFamily(tx);
    op.set_tenant(ws).await?;
    let live = if write {
        op.recheck_session(actor.user_id, actor.credential_id)
            .await?
    } else {
        op.session_is_live(actor.user_id, actor.credential_id)
            .await?
    };
    if !live || !op.workspace_is_live(ws).await? {
        return Ok(Err(CollectionDbError::NotFound));
    }
    Ok(op
        .membership_role(ws, actor.user_id, write)
        .await?
        .ok_or(CollectionDbError::NotFound))
}

pub(crate) async fn family_scope_access(
    tx: &mut FamilyTx,
    ws: Uuid,
    user: Uuid,
    role: WorkspaceRole,
    project: Option<Uuid>,
) -> Result<Option<ScopeAccess>, sqlx::Error> {
    tx.require_tenant(ws)?;
    let Some(project) = project else {
        return Ok(Some(ScopeAccess {
            permission: workspace_base_permission(role),
            archived: false,
        }));
    };
    let rows = tx
        .query(
            "SELECT status FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
            &[Cell::uuid(ws), Cell::uuid(project)],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let archived = row.cell(0)?.string()? == "archived";
    let permission = OperationTx::SqliteFamily(tx)
        .project_permission_by_id(ws, user, project)
        .await?
        .unwrap_or(ProjectPermission::None);
    Ok(Some(ScopeAccess {
        permission,
        archived,
    }))
}

fn family_collection_row(row: &FamilyRow) -> Result<CollectionRow, sqlx::Error> {
    Ok(collection_from((
        row.cell(0)?.id()?,
        row.cell(1)?.id()?,
        row.cell(2)?.optional(Cell::id)?,
        row.cell(3)?.string()?,
        row.cell(4)?.string()?,
        row.cell(5)?.int32()?,
        row.cell(6)?.optional(Cell::datetime)?,
    )))
}

async fn family_find_collection(
    tx: &mut FamilyTx,
    ws: Uuid,
    id: Uuid,
) -> Result<Option<CollectionRow>, sqlx::Error> {
    tx.require_tenant(ws)?;
    tx.query(
        "SELECT id,workspace_id,project_id,kind,name,version,deleted_at FROM collections WHERE workspace_id=?1 AND id=?2",
        &[Cell::uuid(ws), Cell::uuid(id)],
    )
    .await?
    .first()
    .map(family_collection_row)
    .transpose()
}

async fn family_guest_wiki_collection(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: Uuid,
    collection: Uuid,
) -> Result<bool, sqlx::Error> {
    let rows=tx.query("SELECT document_id FROM collection_items WHERE workspace_id=?1 AND collection_id=?2 AND document_id IS NOT NULL", &[Cell::uuid(ws),Cell::uuid(collection)]).await?;
    for row in rows {
        if OperationTx::SqliteFamily(&mut *tx)
            .document_permission(ws, actor, row.cell(0)?.id()?, true)
            .await?
            .at_least(ProjectPermission::View)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) async fn family_require_collection(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    role: WorkspaceRole,
    id: Uuid,
    need: Need,
) -> DbResult<(CollectionRow, ScopeAccess)> {
    let Some(collection) = family_find_collection(tx, ws, id)
        .await?
        .filter(|c| c.deleted_at.is_none())
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    let Some(access) =
        family_scope_access(tx, ws, actor.user_id, role, collection.project_id).await?
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    let readable = access.permission.at_least(ProjectPermission::View)
        || (collection.project_id.is_none()
            && role == WorkspaceRole::Guest
            && family_guest_wiki_collection(tx, ws, actor.user_id, id).await?);
    if !readable {
        return Ok(Err(CollectionDbError::NotFound));
    }
    if need == Need::Edit {
        tx.require_writer()?;
        if !access.permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(CollectionDbError::Forbidden));
        }
        if access.archived {
            return Ok(Err(CollectionDbError::ProjectArchived));
        }
    }
    Ok(Ok((collection, access)))
}

async fn family_audit(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    verb: &str,
    kind: &str,
    id: Uuid,
    payload: Value,
) -> Result<(), sqlx::Error> {
    OperationTx::SqliteFamily(tx)
        .append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(ws),
            actor_user_id: Some(actor.user_id),
            verb: verb.into(),
            target_type: Some(kind.into()),
            target_id: Some(id),
            payload,
            ip: actor.client_ip.map(|ip| ip.to_string()),
        })
        .await
}

pub(crate) async fn family_load_fields(
    tx: &mut FamilyTx,
    ws: Uuid,
    collection: Uuid,
) -> Result<Vec<FieldRow>, sqlx::Error> {
    tx.require_tenant(ws)?;
    let args = [Cell::uuid(ws), Cell::uuid(collection)];
    let fields=tx.query("SELECT id,collection_id,key,name,description,type,version,sort_key,deleted_at FROM collection_fields WHERE workspace_id=?1 AND collection_id=?2 ORDER BY sort_key COLLATE BINARY,id",&args).await?;
    let options=tx.query("SELECT field_id,id,key,label,sort_key,deleted_at FROM collection_options WHERE workspace_id=?1 AND collection_id=?2 ORDER BY sort_key COLLATE BINARY,id",&args).await?;
    fields
        .iter()
        .map(|f| {
            let id = f.cell(0)?.id()?;
            let mut selected = Vec::new();
            for o in &options {
                if o.cell(0)?.id()? == id {
                    selected.push(OptionRow {
                        id: o.cell(1)?.id()?,
                        key: o.cell(2)?.string()?,
                        label: o.cell(3)?.string()?,
                        sort_key: o.cell(4)?.string()?,
                        deleted_at: o.cell(5)?.optional(Cell::datetime)?,
                    });
                }
            }
            Ok(FieldRow {
                id,
                collection_id: f.cell(1)?.id()?,
                key: f.cell(2)?.string()?,
                name: f.cell(3)?.string()?,
                description: f.cell(4)?.optional(Cell::string)?,
                field_type: FieldType::parse(&f.cell(5)?.string()?)
                    .ok_or_else(|| sqlx::Error::Protocol("invalid field type".into()))?,
                version: f.cell(6)?.int32()?,
                sort_key: f.cell(7)?.string()?,
                deleted_at: f.cell(8)?.optional(Cell::datetime)?,
                options: selected,
            })
        })
        .collect()
}

async fn family_upsert_option(
    tx: &mut FamilyTx,
    ws: Uuid,
    collection: Uuid,
    field: Uuid,
    option: &OptionRow,
) -> Result<(), sqlx::Error> {
    tx.query("INSERT INTO collection_options(id,workspace_id,collection_id,field_id,key,label,sort_key,deleted_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO UPDATE SET label=excluded.label,sort_key=excluded.sort_key,deleted_at=excluded.deleted_at",&[Cell::uuid(option.id),Cell::uuid(ws),Cell::uuid(collection),Cell::uuid(field),Cell::text(&option.key),Cell::text(&option.label),Cell::text(&option.sort_key),option.deleted_at.map(|at| Cell::Integer(at.timestamp_micros())).unwrap_or(Cell::Null)]).await?;
    Ok(())
}

pub(crate) async fn family_require_target(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    role: WorkspaceRole,
    target: AttachTarget,
    need: Need,
) -> DbResult<TargetInfo> {
    let (id, table) = match target {
        AttachTarget::Document(id) => (id, "documents"),
        AttachTarget::Task(id) => (id, "tasks"),
    };

    let rows=tx.query(match table {"tasks"=>"SELECT project_id,deleted_at,archived_at FROM tasks WHERE workspace_id=?1 AND id=?2",_=>"SELECT project_id,deleted_at,NULL FROM documents WHERE workspace_id=?1 AND id=?2"},&[Cell::uuid(ws),Cell::uuid(id)]).await?;
    let Some(row) = rows.first() else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if !matches!(row.cell(1)?, Cell::Null) {
        return Ok(Err(CollectionDbError::NotFound));
    }
    let project = row.cell(0)?.optional(Cell::id)?;
    let task_archived = !matches!(row.cell(2)?, Cell::Null);
    let access = match (target, project) {
        (AttachTarget::Document(id), None) => ScopeAccess {
            permission: OperationTx::SqliteFamily(&mut *tx)
                .document_permission(ws, actor.user_id, id, true)
                .await?,
            archived: false,
        },
        _ => match family_scope_access(tx, ws, actor.user_id, role, project).await? {
            Some(a) => a,
            None => return Ok(Err(CollectionDbError::NotFound)),
        },
    };
    if !access.permission.at_least(ProjectPermission::View) {
        return Ok(Err(CollectionDbError::NotFound));
    }
    if need == Need::Edit {
        tx.require_writer()?;
        if !access.permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(CollectionDbError::Forbidden));
        }
        if access.archived {
            return Ok(Err(CollectionDbError::ProjectArchived));
        }
        if task_archived {
            return Ok(Err(CollectionDbError::TaskArchived));
        }
    }
    Ok(Ok(TargetInfo {
        project_id: project,
        can_edit: access.permission.at_least(ProjectPermission::Edit)
            && !access.archived
            && !task_archived,
    }))
}

fn family_item_row(row: &FamilyRow) -> Result<ItemRow, sqlx::Error> {
    Ok(item_from((
        row.cell(0)?.id()?,
        row.cell(1)?.id()?,
        row.cell(2)?.optional(Cell::id)?,
        row.cell(3)?.optional(Cell::id)?,
        row.cell(4)?.int32()?,
    )))
}

async fn family_find_item(
    tx: &mut FamilyTx,
    ws: Uuid,
    target: AttachTarget,
) -> Result<Option<ItemRow>, sqlx::Error> {
    let (id, column) = match target {
        AttachTarget::Document(id) => (id, "document_id"),
        AttachTarget::Task(id) => (id, "task_id"),
    };
    tx.query(match column {"task_id"=>"SELECT id,collection_id,document_id,task_id,version FROM collection_items WHERE workspace_id=?1 AND task_id=?2",_=>"SELECT id,collection_id,document_id,task_id,version FROM collection_items WHERE workspace_id=?1 AND document_id=?2"},&[Cell::uuid(ws),Cell::uuid(id)]).await?.first().map(family_item_row).transpose()
}

async fn family_visible_views(
    tx: &mut FamilyTx,
    ws: Uuid,
    collection: Uuid,
    user: Uuid,
) -> Result<Vec<CollectionViewRow>, sqlx::Error> {
    let rows=tx.query("SELECT id,collection_id,owner_id,visibility,name,type,config,version FROM collection_views WHERE workspace_id=?1 AND collection_id=?2 AND (owner_id=?3 OR visibility='shared')",&[Cell::uuid(ws),Cell::uuid(collection),Cell::uuid(user)]).await?;
    let mut views = rows
        .iter()
        .map(|r| {
            Ok(view_from((
                r.cell(0)?.id()?,
                r.cell(1)?.id()?,
                r.cell(2)?.id()?,
                r.cell(3)?.string()?,
                r.cell(4)?.string()?,
                r.cell(5)?.string()?,
                r.cell(6)?.value()?,
                r.cell(7)?.int32()?,
            )))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    crate::db::tasks::family_sort_names(&mut views, |v| (&v.name, v.id))?;
    Ok(views)
}

async fn family_list_collections(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
) -> DbResult<Vec<CollectionRow>> {
    let role = family_check!(family_begin_member(tx, ws, actor, false).await?);
    let rows=tx.query("SELECT id,workspace_id,project_id,kind,name,version,deleted_at FROM collections WHERE workspace_id=?1 AND deleted_at IS NULL",&[Cell::uuid(ws)]).await?;
    let mut items = Vec::new();
    for row in &rows {
        let collection = family_collection_row(row)?;
        let visible = if collection.project_id.is_some() {
            family_scope_access(tx, ws, actor.user_id, role, collection.project_id)
                .await?
                .is_some_and(|a| a.permission.at_least(ProjectPermission::View))
        } else {
            role != WorkspaceRole::Guest
                || family_guest_wiki_collection(tx, ws, actor.user_id, collection.id).await?
        };
        if visible {
            items.push(collection);
        }
    }
    crate::db::tasks::family_sort_names(&mut items, |c| (&c.name, c.id))?;
    Ok(Ok(items))
}

async fn family_create_collection(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    input: &CollectionCreateInput,
) -> DbResult<CollectionRow> {
    if input.kind == CollectionKind::Task {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    let Some(access) = family_scope_access(tx, ws, actor.user_id, role, input.project_id).await?
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if !access.permission.at_least(ProjectPermission::View) {
        return Ok(Err(CollectionDbError::NotFound));
    }
    if !access.permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(CollectionDbError::Forbidden));
    }
    if access.archived {
        return Ok(Err(CollectionDbError::ProjectArchived));
    }
    let rows=tx.query("INSERT INTO collections(id,workspace_id,project_id,kind,name) VALUES(?1,?2,?3,?4,?5) RETURNING id,workspace_id,project_id,kind,name,version,deleted_at",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(ws),Cell::optional_uuid(input.project_id),Cell::text(input.kind.as_str()),Cell::text(&input.name)]).await?;
    let collection = family_collection_row(rows.first().ok_or(sqlx::Error::RowNotFound)?)?;
    family_audit(tx,ws,actor,"collection.created","collection",collection.id,json!({"name":collection.name,"kind":collection.kind.as_str(),"projectId":collection.project_id})).await?;
    Ok(Ok(collection))
}

async fn family_list_fields(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
) -> DbResult<Vec<FieldRow>> {
    let role = family_check!(family_begin_member(tx, ws, actor, false).await?);
    family_check!(family_require_collection(tx, ws, actor, role, collection, Need::Read).await?);
    Ok(Ok(family_load_fields(tx, ws, collection).await?))
}

async fn family_create_field(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
    input: &FieldCreateInput,
) -> DbResult<FieldRow> {
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    family_check!(family_require_collection(tx, ws, actor, role, collection, Need::Edit).await?);
    let fields = family_load_fields(tx, ws, collection).await?;
    if fields.len() >= FIELDS_PER_COLLECTION_MAX {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let key = input
        .key
        .clone()
        .unwrap_or_else(|| format!("f_{}", fields.len() + 1));
    if fields.iter().any(|f| f.key == key) {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let id = Uuid::now_v7();
    tx.query("INSERT INTO collection_fields(id,workspace_id,collection_id,key,name,description,type,sort_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",&[Cell::uuid(id),Cell::uuid(ws),Cell::uuid(collection),Cell::text(&key),Cell::text(&input.name),Cell::optional_text(input.description.as_deref()),Cell::text(input.field_type.as_str()),Cell::text(pad_sort_key(fields.len()))]).await?;
    for (index, label) in input.options.iter().enumerate() {
        family_upsert_option(
            tx,
            ws,
            collection,
            id,
            &OptionRow {
                id: Uuid::now_v7(),
                key: format!("o_{}", index + 1),
                label: label.clone(),
                sort_key: pad_sort_key(index),
                deleted_at: None,
            },
        )
        .await?;
    }
    family_audit(
        tx,
        ws,
        actor,
        "collection_field.created",
        "collection_field",
        id,
        json!({"collectionId":collection,"key":key,"type":input.field_type.as_str()}),
    )
    .await?;
    Ok(family_load_fields(tx, ws, collection)
        .await?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or(CollectionDbError::NotFound))
}

async fn family_attach_item(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
    target: AttachTarget,
) -> DbResult<ItemRow> {
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    let (scope, _) = family_check!(
        family_require_collection(tx, ws, actor, role, collection, Need::Edit).await?
    );
    if !matches!(
        (scope.kind, target),
        (CollectionKind::Document, AttachTarget::Document(_))
            | (CollectionKind::Task, AttachTarget::Task(_))
    ) {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let info = family_check!(family_require_target(tx, ws, actor, role, target, Need::Edit).await?);
    if info.project_id != scope.project_id {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    if let Some(existing) = family_find_item(tx, ws, target).await? {
        return Ok(if existing.collection_id == collection {
            Ok(existing)
        } else {
            Err(CollectionDbError::InvalidInput)
        });
    }
    let (doc, task) = match target {
        AttachTarget::Document(id) => (Some(id), None),
        AttachTarget::Task(id) => (None, Some(id)),
    };
    let rows=tx.query("INSERT INTO collection_items(id,workspace_id,collection_id,document_id,task_id) VALUES(?1,?2,?3,?4,?5) RETURNING id,collection_id,document_id,task_id,version",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(ws),Cell::uuid(collection),Cell::optional_uuid(doc),Cell::optional_uuid(task)]).await?;
    Ok(Ok(family_item_row(
        rows.first().ok_or(sqlx::Error::RowNotFound)?,
    )?))
}

async fn family_put_value(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
    item_id: Uuid,
    input: &ValueInput,
) -> DbResult<i32> {
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    let (_, access) = family_check!(
        family_require_collection(tx, ws, actor, role, collection, Need::Read).await?
    );
    if access.archived {
        return Ok(Err(CollectionDbError::ProjectArchived));
    }
    let rows=tx.query("SELECT id,collection_id,document_id,task_id,version FROM collection_items WHERE workspace_id=?1 AND id=?2 AND collection_id=?3",&[Cell::uuid(ws),Cell::uuid(item_id),Cell::uuid(collection)]).await?;
    let Some(row) = rows.first() else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    let item = family_item_row(row)?;
    let target = match (item.document_id, item.task_id) {
        (Some(id), _) => AttachTarget::Document(id),
        (_, Some(id)) => AttachTarget::Task(id),
        _ => return Ok(Err(CollectionDbError::NotFound)),
    };
    family_check!(family_require_target(tx, ws, actor, role, target, Need::Edit).await?);
    if item.version != input.expected_version {
        return Ok(Err(CollectionDbError::VersionConflict));
    }
    let Some(field) = family_load_fields(tx, ws, collection)
        .await?
        .into_iter()
        .find(|f| f.id == input.field_id && f.deleted_at.is_none())
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if field.version != input.expected_field_version {
        return Ok(Err(CollectionDbError::VersionConflict));
    }
    if !input.value.fits(field.field_type) {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    match &input.value {
        CollectionValue::Options(ids) => {
            if ids.iter().collect::<HashSet<_>>().len() != ids.len()
                || (field.field_type == FieldType::Select && ids.len() > 1)
            {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
            let previous=tx.query("SELECT option_id FROM collection_choices WHERE workspace_id=?1 AND item_id=?2 AND field_id=?3",&[Cell::uuid(ws),Cell::uuid(item_id),Cell::uuid(field.id)]).await?.iter().map(|r|r.cell(0)?.id()).collect::<Result<Vec<_>,sqlx::Error>>()?;
            if ids.iter().any(|id| {
                !field
                    .options
                    .iter()
                    .any(|o| o.id == *id && (o.deleted_at.is_none() || previous.contains(id)))
            }) {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
        }
        CollectionValue::Users(ids) => {
            if ids.iter().collect::<HashSet<_>>().len() != ids.len()
                || (field.field_type == FieldType::User && ids.len() > 1)
            {
                return Ok(Err(CollectionDbError::InvalidInput));
            }
            for id in ids {
                if tx
                    .query(
                        "SELECT 1 FROM memberships WHERE workspace_id=?1 AND user_id=?2",
                        &[Cell::uuid(ws), Cell::uuid(*id)],
                    )
                    .await?
                    .is_empty()
                {
                    return Ok(Err(CollectionDbError::InvalidInput));
                }
            }
        }
        _ => {}
    }
    for table in [
        "collection_values",
        "collection_choices",
        "collection_people",
    ] {
        tx.query(
            match table {"collection_values"=>"DELETE FROM collection_values WHERE workspace_id=?1 AND item_id=?2 AND field_id=?3","collection_choices"=>"DELETE FROM collection_choices WHERE workspace_id=?1 AND item_id=?2 AND field_id=?3","collection_people"=>"DELETE FROM collection_people WHERE workspace_id=?1 AND item_id=?2 AND field_id=?3",_=>unreachable!()},
            &[Cell::uuid(ws), Cell::uuid(item_id), Cell::uuid(field.id)],
        )
        .await?;
    }
    let args = [
        Cell::uuid(ws),
        Cell::uuid(collection),
        Cell::uuid(item_id),
        Cell::uuid(field.id),
        Cell::text(field.field_type.as_str()),
    ];
    match &input.value {
        CollectionValue::Null => {}
        CollectionValue::Options(ids) | CollectionValue::Users(ids) => {
            let (table, _column) = if matches!(input.value, CollectionValue::Users(_)) {
                ("collection_people", "user_id")
            } else {
                ("collection_choices", "option_id")
            };
            for id in ids {
                let mut values = args.to_vec();
                values.push(Cell::uuid(*id));
                tx.query(match table {"collection_people"=>"INSERT INTO collection_people(workspace_id,collection_id,item_id,field_id,field_type,user_id) VALUES(?1,?2,?3,?4,?5,?6)",_=>"INSERT INTO collection_choices(workspace_id,collection_id,item_id,field_id,field_type,option_id) VALUES(?1,?2,?3,?4,?5,?6)"},&values).await?;
            }
        }
        scalar => {
            let (column, value) = match scalar {
                CollectionValue::Text(s) => ("value_text", Cell::text(s)),
                CollectionValue::Number(n) => (
                    "value_number",
                    Cell::text(crate::db::view_query::format_number(*n)),
                ),
                CollectionValue::Date(d) => ("value_date", Cell::text(d.to_string())),
                // SQLx's PostgreSQL timestamptz bind has microsecond precision.
                CollectionValue::Datetime(at) => ("value_ts", Cell::Integer(at.timestamp_micros())),
                CollectionValue::Checkbox(b) => ("value_bool", Cell::Integer(i64::from(*b))),
                _ => unreachable!(),
            };
            let mut values = args.to_vec();
            values.push(value);
            tx.query(match column {"value_text"=>"INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_text) VALUES(?1,?2,?3,?4,?5,?6)","value_number"=>"INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_number) VALUES(?1,?2,?3,?4,?5,?6)","value_date"=>"INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_date) VALUES(?1,?2,?3,?4,?5,?6)","value_ts"=>"INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_ts) VALUES(?1,?2,?3,?4,?5,?6)","value_bool"=>"INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_bool) VALUES(?1,?2,?3,?4,?5,?6)",_=>unreachable!()},&values).await?;
        }
    }
    tx.query("UPDATE collection_items SET version=version+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(ws),Cell::uuid(item_id)]).await?;
    Ok(Ok(item.version + 1))
}

async fn family_item_for_target(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    target: AttachTarget,
) -> DbResult<ItemLookup> {
    let role = family_check!(family_begin_member(tx, ws, actor, false).await?);
    let info = family_check!(family_require_target(tx, ws, actor, role, target, Need::Read).await?);
    let item = family_find_item(tx, ws, target).await?;
    let values = match &item {
        Some(i) => crate::db::collection_query::family_load_item_values(tx, ws, &[i.id])
            .await?
            .into_iter()
            .map(|(_, f, v)| (f, v))
            .collect(),
        None => Vec::new(),
    };
    Ok(Ok(ItemLookup {
        item,
        values,
        can_edit: info.can_edit,
    }))
}

async fn family_project_collection(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    project: Uuid,
) -> DbResult<ProjectCollection> {
    let role = family_check!(family_begin_member(tx, ws, actor, false).await?);
    let Some(access) = family_scope_access(tx, ws, actor.user_id, role, Some(project))
        .await?
        .filter(|a| a.permission.at_least(ProjectPermission::View))
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    let rows=tx.query("SELECT id,workspace_id,project_id,kind,name,version,deleted_at FROM collections WHERE workspace_id=?1 AND project_id=?2 AND kind='task'",&[Cell::uuid(ws),Cell::uuid(project)]).await?;
    let Some(row) = rows.first() else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    Ok(Ok(ProjectCollection {
        collection: family_collection_row(row)?,
        can_edit: access.permission.at_least(ProjectPermission::Edit) && !access.archived,
        can_manage: access.permission == ProjectPermission::Manage,
    }))
}

async fn family_list_views(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
) -> DbResult<ViewList> {
    let role = family_check!(family_begin_member(tx, ws, actor, false).await?);
    let (_, access) = family_check!(
        family_require_collection(tx, ws, actor, role, collection, Need::Read).await?
    );
    let items = family_visible_views(tx, ws, collection, actor.user_id).await?;
    Ok(Ok(ViewList {
        can_save: !access.archived,
        can_manage: !access.archived && access.permission == ProjectPermission::Manage,
        items,
    }))
}

async fn family_patch_field(
    tx: &mut FamilyTx,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    field_id: Uuid,
    input: &FieldPatchInput,
) -> DbResult<FieldRow> {
    let role = family_check!(family_begin_member(tx, workspace_id, actor, true).await?);
    family_check!(
        family_require_collection(tx, workspace_id, actor, role, collection_id, Need::Edit).await?
    );
    let fields = family_load_fields(tx, workspace_id, collection_id).await?;
    let Some(field) = fields.into_iter().find(|field| field.id == field_id) else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if field.version != input.expected_version {
        return Ok(Err(CollectionDbError::VersionConflict));
    }
    let mut options = field.options.clone();
    if let Some(patches) = &input.options {
        if !field.field_type.has_options() {
            return Ok(Err(CollectionDbError::InvalidInput));
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
            return Ok(Err(CollectionDbError::InvalidInput));
        }
        // Option keys are unique per field: new options continue after the
        // highest key ever used so a revived slot never collides.
        let mut last_key = field
            .options
            .iter()
            .filter_map(|option| option.key.strip_prefix("o_")?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let now = crate::db::tasks::family_clock(tx).await?;
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
    tx.query("UPDATE collection_fields SET name=?3,description=?4,deleted_at=CASE WHEN ?5 THEN COALESCE(deleted_at,(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)) ELSE NULL END,version=version+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(workspace_id),Cell::uuid(field_id),Cell::text(input.name.as_deref().unwrap_or(&field.name)),Cell::optional_text(description.as_deref()),Cell::Integer(i64::from(deleted))]).await?;
    if input.options.is_some() {
        for option in &options {
            family_upsert_option(tx, workspace_id, collection_id, field_id, option).await?;
        }
    }
    family_audit(
        tx,
        workspace_id,
        actor,
        "collection_field.updated",
        "collection_field",
        field_id,
        json!({"collectionId": collection_id, "deleted": deleted, "version": field.version + 1}),
    )
    .await?;
    let updated = family_load_fields(tx, workspace_id, collection_id)
        .await?
        .into_iter()
        .find(|row| row.id == field_id);
    Ok(updated.ok_or(CollectionDbError::NotFound))
}

pub(crate) async fn family_validate_query_config(
    collection: &CollectionRow,
    fields: &[FieldRow],
    config: &QueryConfig,
    day_given: bool,
    window: Option<&CalendarWindow>,
) -> DbResult<()> {
    let live = |id| fields.iter().find(|f| f.id == id && f.deleted_at.is_none());
    match config.group_by {
        Some(GroupBy::Status) if collection.kind != CollectionKind::Task => {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        Some(GroupBy::Field(id))
            if !live(id).is_some_and(|f| f.field_type == FieldType::Select) =>
        {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        _ => {}
    }
    match config.date_by {
        Some(DateBy::Due | DateBy::Start) if collection.kind != CollectionKind::Task => {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        Some(DateBy::Field(id))
            if !live(id)
                .is_some_and(|f| matches!(f.field_type, FieldType::Date | FieldType::Datetime)) =>
        {
            return Ok(Err(CollectionDbError::InvalidInput))
        }
        _ => {}
    }
    if day_given && window.is_none() {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    if let Some(w) = window {
        if config.date_by.is_none()
            || w.from >= w.to
            || (w.to - w.from).num_days() > WINDOW_DAYS_MAX
            || !crate::db::tasks::family_collection_zone_known(&w.time_zone)?
        {
            return Ok(Err(CollectionDbError::InvalidInput));
        }
    }
    Ok(Ok(()))
}

async fn family_save_view(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    view_id: Option<Uuid>,
    input: &CollectionViewInput,
) -> DbResult<CollectionViewRow> {
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    let (collection, access) = family_check!(
        family_require_collection(tx, ws, actor, role, collection_id, Need::Read).await?
    );
    if access.archived {
        return Ok(Err(CollectionDbError::ProjectArchived));
    }
    let fields = family_load_fields(tx, ws, collection_id).await?;
    family_check!(
        family_validate_query_config(&collection, &fields, &input.config, false, None).await?
    );
    if crate::db::view_query::prepare_selected_collection_view(
        tx,
        ViewScope {
            workspace_id: ws,
            project_id: collection.project_id,
            collection_id: Some(collection_id),
            kind: collection.root_kind(),
        },
        &input.config.query,
    )
    .await?
    .is_err()
    {
        return Ok(Err(CollectionDbError::InvalidInput));
    }
    let manage = access.permission == ProjectPermission::Manage;
    if input.visibility == Visibility::Shared && !manage {
        return Ok(Err(CollectionDbError::Forbidden));
    }
    let rows = if let Some(id) = view_id {
        let Some(old) = family_visible_views(tx, ws, collection_id, actor.user_id)
            .await?
            .into_iter()
            .find(|v| v.id == id)
        else {
            return Ok(Err(CollectionDbError::NotFound));
        };
        if old.visibility != input.visibility.as_str() && old.owner_id != actor.user_id {
            return Ok(Err(CollectionDbError::InvalidInput));
        }
        if old.visibility == "shared" && !manage {
            return Ok(Err(CollectionDbError::Forbidden));
        }
        if old.visibility != "shared" && old.owner_id != actor.user_id {
            return Ok(Err(CollectionDbError::NotFound));
        }
        let Some(expected) = input.expected_version else {
            return Ok(Err(CollectionDbError::InvalidInput));
        };
        tx.query("UPDATE collection_views SET name=?4,type=?5,visibility=?6,config=?7,version=version+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2 AND version=?3 RETURNING id,collection_id,owner_id,visibility,name,type,config,version",&[Cell::uuid(ws),Cell::uuid(id),Cell::Integer(i64::from(expected)),Cell::text(&input.name),Cell::text(input.view_type.as_str()),Cell::text(input.visibility.as_str()),Cell::json(&input.config.to_json())?]).await?
    } else {
        tx.query("INSERT INTO collection_views(id,workspace_id,collection_id,owner_id,visibility,name,type,config) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) RETURNING id,collection_id,owner_id,visibility,name,type,config,version",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(ws),Cell::uuid(collection_id),Cell::uuid(actor.user_id),Cell::text(input.visibility.as_str()),Cell::text(&input.name),Cell::text(input.view_type.as_str()),Cell::json(&input.config.to_json())?]).await?
    };
    let Some(r) = rows.first() else {
        return Ok(Err(CollectionDbError::VersionConflict));
    };
    let view = view_from((
        r.cell(0)?.id()?,
        r.cell(1)?.id()?,
        r.cell(2)?.id()?,
        r.cell(3)?.string()?,
        r.cell(4)?.string()?,
        r.cell(5)?.string()?,
        r.cell(6)?.value()?,
        r.cell(7)?.int32()?,
    ));
    if view.visibility == "shared" {
        family_audit(
            tx,
            ws,
            actor,
            if view_id.is_some() {
                "collection_view.updated"
            } else {
                "collection_view.created"
            },
            "collection_view",
            view.id,
            json!({"collectionId":collection_id,"name":view.name,"version":view.version}),
        )
        .await?;
    }
    Ok(Ok(view))
}

async fn family_remove_view(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: &Actor,
    collection: Uuid,
    id: Uuid,
) -> DbResult<()> {
    let role = family_check!(family_begin_member(tx, ws, actor, true).await?);
    let (_, access) = family_check!(
        family_require_collection(tx, ws, actor, role, collection, Need::Read).await?
    );
    if access.archived {
        return Ok(Err(CollectionDbError::ProjectArchived));
    }
    let Some(old) = family_visible_views(tx, ws, collection, actor.user_id)
        .await?
        .into_iter()
        .find(|v| v.id == id)
    else {
        return Ok(Err(CollectionDbError::NotFound));
    };
    if old.visibility == "shared" && access.permission != ProjectPermission::Manage {
        return Ok(Err(CollectionDbError::Forbidden));
    }
    if old.visibility != "shared" && old.owner_id != actor.user_id {
        return Ok(Err(CollectionDbError::NotFound));
    }
    tx.query(
        "DELETE FROM collection_views WHERE workspace_id=?1 AND collection_id=?2 AND id=?3",
        &[Cell::uuid(ws), Cell::uuid(collection), Cell::uuid(id)],
    )
    .await?;
    if old.visibility == "shared" {
        family_audit(
            tx,
            ws,
            actor,
            "collection_view.deleted",
            "collection_view",
            id,
            json!({"collectionId":collection,"name":old.name}),
        )
        .await?;
    }
    Ok(Ok(()))
}

/// Explicit official-sqld controls, outside registered remote cohorts.
#[cfg(all(test, feature = "db-tests"))]
mod port_data_sdk_tests {
    use super::*;
    use crate::db::{backend::RemoteDatabase, libsql_finish_fixture::LibsqlFinishFixture};
    use futures_util::FutureExt;
    use std::{num::NonZeroU32, path::PathBuf, sync::Arc};

    async fn backend(driver: &LibsqlFinishFixture) -> Backend {
        Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            NonZeroU32::new(1).unwrap(),
        )))
    }

    #[tokio::test]
    async fn selected_port_data_sdk_precision_export_secrets_and_refusals() {
        let sqld = PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("explicit official sqld required"),
        );
        let root = std::env::temp_dir().join(format!("port-data-sdk-{}", Uuid::now_v7()));
        let driver = LibsqlFinishFixture::start(&sqld, &root).await.unwrap();
        let db = backend(&driver).await;
        let body=std::panic::AssertUnwindSafe(async {
            crate::db::migrate::initialize_family_backend_for_test(&db).await.unwrap();
            let ws=Uuid::now_v7();let user=Uuid::now_v7();let credential=Uuid::now_v7();let doc=Uuid::now_v7();
            let mut tx=db.begin_write().await.unwrap();
            let DbTransaction::SqliteFamily(family)=&mut tx else{unreachable!()};
            family.execute("INSERT INTO workspaces(id,slug,name) VALUES(?1,'data-sdk','Data SDK')",&[Cell::uuid(ws)]).await.unwrap();
            family.execute("INSERT INTO users(id,email,given_name) VALUES(?1,'data-sdk@example.invalid','Data SDK')",&[Cell::uuid(user)]).await.unwrap();
            family.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",&[Cell::uuid(ws),Cell::uuid(user)]).await.unwrap();
            family.execute("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'synthetic-data-sdk',unixepoch()*1000000+3600000000)",&[Cell::uuid(credential),Cell::uuid(user)]).await.unwrap();
            family.execute("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'SDK document',?3,'a',1,'published',2,?4,'{}')",&[Cell::uuid(doc),Cell::uuid(ws),Cell::text(doc.simple().to_string()),Cell::uuid(user)]).await.unwrap();
            tx.commit().await.unwrap();
            let actor=Actor{user_id:user,credential_id:credential,client_ip:None};
            let collection=create_collection_backend(&db,ws,&actor,&CollectionCreateInput{name:"SDK".into(),kind:CollectionKind::Document,project_id:None}).await.unwrap().unwrap();
            let field=create_field_backend(&db,ws,&actor,collection.id,&crate::collections::parse_field_create(&json!({"name":"Decimal","type":"number"})).unwrap()).await.unwrap().unwrap();
            let item=attach_item_backend(&db,ws,&actor,collection.id,AttachTarget::Document(doc)).await.unwrap().unwrap();
            let input=crate::collections::parse_value_input(&json!({"fieldId":field.id,"expectedVersion":1,"expectedFieldVersion":1,"value":{"number":0.000000000000000123456789}})).unwrap();
            assert_eq!(put_value_backend(&db,ws,&actor,collection.id,item.id,&input).await.unwrap().unwrap(),2);
            let fresh=backend(&driver).await;
            let lookup=item_for_target_backend(&fresh,ws,&actor,AttachTarget::Document(doc)).await.unwrap().unwrap();
            assert_eq!(lookup.values,vec![(field.id,json!({"number":0.000000000000000123456789}))]);
            assert_eq!(put_value_backend(&fresh,ws,&actor,collection.id,item.id,&input).await.unwrap(),Err(CollectionDbError::VersionConflict));
            assert!(matches!(list_fields_backend(&fresh,Uuid::now_v7(),&actor,collection.id).await.unwrap(),Err(CollectionDbError::NotFound)));
            let snapshot=crate::db::workspace_export::load_export_snapshot_backend(&fresh,ws,user,credential).await.unwrap().unwrap();
            assert_eq!(snapshot.documents.len(),1);
            let report=crate::secret_verify::verify_sealed_secrets_backend(&fresh,None).await.unwrap();
            assert_eq!(report.user_mfa.checked,0);
            let mut tx=fresh.begin_write().await.unwrap();
            let DbTransaction::SqliteFamily(family)=&mut tx else{unreachable!()};
            family.execute("UPDATE sessions SET revoked_at=1 WHERE id=?1",&[Cell::uuid(credential)]).await.unwrap();tx.commit().await.unwrap();
            assert!(matches!(put_value_backend(&fresh,ws,&actor,collection.id,item.id,&input).await.unwrap(),Err(CollectionDbError::NotFound)));
            fresh.close().await.unwrap();
        }).catch_unwind().await;
        let close = db.close().await;
        let finish = driver.finish().await;
        if let Err(primary) = body {
            assert!(close.is_ok());
            assert!(finish.is_ok());
            std::panic::resume_unwind(primary);
        }
        close.unwrap();
        assert!(finish.unwrap().sqld_reaped);
        std::fs::remove_dir_all(root).unwrap();
    }
}
