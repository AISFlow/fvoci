//! Short current-actor/generation-fenced commits. Network work holds no DB lock.
use crate::api::zotero_dto::*;
use crate::auth::password::Keyring;
use crate::db::context::{
    lock_membership_users, lock_tree, recheck_session, set_self_user, set_tenant,
};
use crate::db::documents::{create_wiki_document_tx, CreateDocumentInput};
use crate::db::personal_input::owns_personal_workspace;
use crate::db::task_origins::{document_view_permission, task_view_permission};
use crate::integrations::zotero::{self, Collection, Deleted, Item, Library, ZoteroError};
use crate::projects::ProjectPermission;
use crate::secret_box;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Index workers already hold system context. Explicit identity joins remain
/// necessary there; query hydration additionally supplies the current actor.
pub(crate) async fn private_search_texts(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor: Option<(Uuid, Uuid)>,
    documents: &[Uuid],
) -> Result<BTreeMap<Uuid, String>, sqlx::Error> {
    if documents.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows:Vec<(Uuid,serde_json::Value)>=sqlx::query_as("SELECT r.document_id,r.bibliography FROM fvoci.zotero_references r JOIN fvoci.documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id AND d.deleted_at IS NULL JOIN fvoci.users u ON u.id=r.owner_user_id AND u.personal_workspace_id=r.workspace_id AND u.deleted_at IS NULL JOIN fvoci.workspaces w ON w.id=r.workspace_id AND w.kind='personal' AND w.deleted_at IS NULL JOIN fvoci.memberships m ON m.workspace_id=r.workspace_id AND m.user_id=r.owner_user_id AND m.role='owner' WHERE r.workspace_id=$1 AND r.document_id=ANY($2) AND ($3::uuid IS NULL OR (r.owner_user_id=$3 AND EXISTS(SELECT 1 FROM fvoci.sessions s WHERE s.user_id=$3 AND s.id=$4 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp())))").bind(workspace).bind(documents).bind(actor.map(|a|a.0)).bind(actor.map(|a|a.1)).fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(|(id, value)| (id, bibliographic_text(value)))
        .collect())
}

/// The generic search credential guard also supports PATs. Private metadata
/// requires this separate proof of an actual current browser session row.
pub(crate) async fn cookie_session_is_live(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    session: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.sessions s JOIN fvoci.users u ON u.id=s.user_id WHERE s.id=$2 AND s.user_id=$1 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp() AND u.deleted_at IS NULL AND u.suspended_at IS NULL)").bind(actor).bind(session).fetch_one(&mut **tx).await
}
pub(crate) fn bibliographic_text(value: serde_json::Value) -> String {
    if value.to_string().len() > zotero::BODY_MAX {
        return String::new();
    }
    let Ok(b) = serde_json::from_value::<Bibliography>(value) else {
        return String::new();
    };
    let mut parts = vec![b.title.chars().take(16384).collect::<String>()];
    for creator in b.creators.into_iter().take(50) {
        for value in [creator.name, creator.first_name, creator.last_name]
            .into_iter()
            .flatten()
        {
            parts.push(value.chars().take(1024).collect());
        }
    }
    if let Some((fields, _)) = zotero::schema(&b.item_type) {
        for (key, value) in b.fields {
            if fields.contains(&key.as_str()) {
                parts.push(value.chars().take(16384).collect());
            }
        }
    }
    parts.join("\n").chars().take(32768).collect()
}

/// Existing project grant writers take the parent row lock; bind the final
/// link write to the same serialization, including archived/deleted targets.
async fn link_permission(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor,
    id: Uuid,
    task: bool,
) -> Result<ProjectPermission, DbError> {
    let sql = if task {
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
    } else {
        "SELECT project_id FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
    };
    let project: Option<Option<Uuid>> = sqlx::query_scalar(sql)
        .bind(actor.workspace)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(project) = project else {
        return Ok(ProjectPermission::None);
    };
    if let Some(project) = project {
        let Some((permission, archived)) = crate::db::projects::share_lock_project_permission(
            tx,
            actor.workspace,
            actor.user,
            project,
        )
        .await?
        else {
            return Ok(ProjectPermission::None);
        };
        if archived {
            return Ok(ProjectPermission::None);
        }
        return Ok(permission);
    }
    if task {
        Ok(ProjectPermission::None)
    } else {
        Ok(document_view_permission(tx, actor.workspace, actor.user, id).await?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error")]
    Sql(#[from] sqlx::Error),
    #[error("Zotero resource not found")]
    NotFound,
    #[error("Zotero command conflicts with current state")]
    Conflict,
    #[error("encryption key unavailable")]
    Encryption,
    #[error(transparent)]
    Remote(#[from] ZoteroError),
}
#[derive(Clone, Copy)]
pub struct Actor {
    pub workspace: Uuid,
    pub user: Uuid,
    pub session: Uuid,
}
async fn begin(pool: &PgPool, actor: Actor) -> Result<Transaction<'static, Postgres>, DbError> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, actor.workspace).await?;
    set_self_user(&mut tx, actor.user).await?;
    lock_membership_users(&mut tx, &[actor.user]).await?;
    if !recheck_session(&mut tx, actor.user, actor.session).await?
        || !owns_personal_workspace(&mut tx, actor.workspace, actor.user).await?
    {
        return Err(DbError::NotFound);
    }
    lock_tree(&mut tx, actor.workspace).await?;
    Ok(tx)
}
fn connector_output(row: &sqlx::postgres::PgRow) -> Result<ConnectorOutput, DbError> {
    let kind: String = row.try_get("library_type")?;
    let kind = match kind.as_str() {
        "user" => LibraryType::User,
        "group" => LibraryType::Group,
        _ => return Err(ZoteroError::Invalid.into()),
    };
    let retry: Option<DateTime<Utc>> = row.try_get("retry_at")?;
    Ok(ConnectorOutput {
        id: row.try_get("id")?,
        library_type: kind,
        remote_library_id: row.try_get::<i64, _>("remote_library_id")?.to_string(),
        library_url: row.try_get("library_url")?,
        state: row.try_get("state")?,
        generation: row.try_get::<i64, _>("generation")?.to_string(),
        completed_version: row.try_get::<i64, _>("completed_version")?.to_string(),
        progress_version: row
            .try_get::<Option<i64>, _>("progress_version")?
            .map(|v| v.to_string()),
        committed_pages: row.try_get("committed_pages")?,
        retry_at: retry.map(|v| v.to_rfc3339()),
        reconciliation_required: row.try_get("reconciliation_required")?,
    })
}
async fn connector(
    tx: &mut Transaction<'_, Postgres>,
    actor: Actor,
    id: Uuid,
) -> Result<ConnectorOutput, DbError> {
    let row=sqlx::query("SELECT * FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3 FOR UPDATE")
        .bind(actor.workspace).bind(actor.user).bind(id).fetch_optional(&mut **tx).await?.ok_or(DbError::NotFound)?;
    connector_output(&row)
}
pub async fn list(pool: &PgPool, actor: Actor) -> Result<ConnectorListOutput, DbError> {
    let mut tx = begin(pool, actor).await?;
    let rows=sqlx::query("SELECT * FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 ORDER BY id LIMIT 21")
        .bind(actor.workspace).bind(actor.user).fetch_all(&mut *tx).await?;
    if rows.len() > 20 {
        return Err(ZoteroError::Limit.into());
    }
    let connectors = rows
        .iter()
        .map(connector_output)
        .collect::<Result<_, _>>()?;
    tx.commit().await?;
    Ok(ConnectorListOutput { connectors })
}
pub async fn connect(
    pool: &PgPool,
    actor: Actor,
    input: &ConnectBody,
    keys: &Keyring,
) -> Result<ConnectorOutput, DbError> {
    let library = Library::new(
        input.library_type,
        &input.remote_library_id,
        &input.library_url,
    )?;
    if input.api_key.is_empty()
        || input.api_key.len() > 256
        || !input
            .api_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(ZoteroError::Invalid.into());
    }
    let mut tx = begin(pool, actor).await?;
    // All connector mutations share the current actor/tree lock in begin().
    // Replacing a credential or choosing another library cannot bypass a
    // deadline learned from the fixed upstream host for this owner.
    let retry: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT max(retry_at) FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2")
        .bind(actor.workspace).bind(actor.user).fetch_one(&mut *tx).await?;
    let existing:Option<Uuid>=sqlx::query_scalar("SELECT id FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND library_type=$3 AND remote_library_id=$4 FOR UPDATE")
        .bind(actor.workspace).bind(actor.user).bind(library.kind.as_str()).bind(library.remote_id).fetch_optional(&mut *tx).await?;
    let id = existing.unwrap_or_else(Uuid::now_v7);
    let sealed = secret_box::seal(
        keys,
        &input.api_key,
        &zotero::secret_context(actor.workspace, actor.user, id),
    )
    .map_err(|_| DbError::Encryption)?;
    if existing.is_some() {
        // Credential retirement fences commits immediately, but an old HTTP
        // read still owns its host lease until its future has ended.
        sqlx::query("UPDATE fvoci.zotero_connectors SET library_url=$4,state='connected',generation=generation+1,reconciliation_required=true,progress_version=NULL,committed_pages=0,retry_at=$5,updated_at=now() WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3")
            .bind(actor.workspace).bind(actor.user).bind(id).bind(&library.website).bind(retry).execute(&mut *tx).await?;
    } else {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2").bind(actor.workspace).bind(actor.user).fetch_one(&mut *tx).await?;
        if count >= 20 {
            return Err(ZoteroError::Limit.into());
        }
        sqlx::query("INSERT INTO fvoci.zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url,retry_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(id).bind(actor.workspace).bind(actor.user).bind(library.kind.as_str()).bind(library.remote_id).bind(&library.website).bind(retry).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO fvoci.zotero_credentials(connector_id,workspace_id,owner_user_id,sealed_key) VALUES($1,$2,$3,$4) ON CONFLICT(connector_id) DO UPDATE SET sealed_key=EXCLUDED.sealed_key")
        .bind(id).bind(actor.workspace).bind(actor.user).bind(sealed).execute(&mut *tx).await?;
    let output = connector(&mut tx, actor, id).await?;
    tx.commit().await?;
    Ok(output)
}
pub async fn disconnect(pool: &PgPool, actor: Actor, id: Uuid) -> Result<ConnectorOutput, DbError> {
    let mut tx = begin(pool, actor).await?;
    connector(&mut tx, actor, id).await?;
    sqlx::query("DELETE FROM fvoci.zotero_credentials WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3")
        .bind(actor.workspace).bind(actor.user).bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE fvoci.zotero_connectors SET state='disconnected',generation=generation+1,updated_at=now() WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3")
        .bind(actor.workspace).bind(actor.user).bind(id).execute(&mut *tx).await?;
    let output = connector(&mut tx, actor, id).await?;
    tx.commit().await?;
    Ok(output)
}
// No Debug or Serialize: a cycle contains the opened write-only credential.
pub struct Cycle {
    pub actor: Actor,
    pub connector: Uuid,
    pub generation: i64,
    pub sync_id: Uuid,
    pub since: i64,
    pub library: Library,
    pub key: String,
}
pub async fn start(
    pool: &PgPool,
    actor: Actor,
    id: Uuid,
    keys: &Keyring,
) -> Result<Cycle, DbError> {
    let mut tx = begin(pool, actor).await?;
    let output = connector(&mut tx, actor, id).await?;
    if output.state != "connected" {
        return Err(ZoteroError::Retired.into());
    }
    let ready:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND (retry_at>now() OR (sync_id IS NOT NULL AND sync_expires_at>now())))")
        .bind(actor.workspace).bind(actor.user).fetch_one(&mut *tx).await?;
    if !ready {
        return Err(DbError::Conflict);
    }
    let sealed:String=sqlx::query_scalar("SELECT sealed_key FROM fvoci.zotero_credentials WHERE connector_id=$1 AND workspace_id=$2 AND owner_user_id=$3")
        .bind(id).bind(actor.workspace).bind(actor.user).fetch_optional(&mut *tx).await?.ok_or(DbError::Encryption)?;
    let key = secret_box::open(
        keys,
        &sealed,
        &zotero::secret_context(actor.workspace, actor.user, id),
    )
    .map_err(|_| DbError::Encryption)?;
    let sync_id = Uuid::now_v7();
    sqlx::query("UPDATE fvoci.zotero_connectors SET sync_id=$2,sync_expires_at=now()+interval '65 seconds',progress_version=NULL,committed_pages=0 WHERE id=$1").bind(id).bind(sync_id).execute(&mut *tx).await?;
    let cycle = Cycle {
        actor,
        connector: id,
        generation: zotero::decimal(&output.generation)?,
        sync_id,
        since: if output.reconciliation_required {
            0
        } else {
            zotero::decimal(&output.completed_version)?
        },
        library: Library::new(
            output.library_type,
            &output.remote_library_id,
            &output.library_url,
        )?,
        key,
    };
    tx.commit().await?;
    Ok(cycle)
}
async fn fence(tx: &mut Transaction<'_, Postgres>, cycle: &Cycle) -> Result<(), DbError> {
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3 AND generation=$4 AND sync_id=$5 AND state='connected' AND sync_expires_at>now())")
        .bind(cycle.actor.workspace).bind(cycle.actor.user).bind(cycle.connector).bind(cycle.generation).bind(cycle.sync_id).fetch_one(&mut **tx).await?;
    if !valid {
        return Err(ZoteroError::Retired.into());
    }
    Ok(())
}
pub async fn current(pool: &PgPool, cycle: &Cycle) -> Result<(), DbError> {
    let mut tx = begin(pool, cycle.actor).await?;
    connector(&mut tx, cycle.actor, cycle.connector).await?;
    fence(&mut tx, cycle).await?;
    tx.commit().await?;
    Ok(())
}
/// Called only after this cycle's outbound read future has ended. Generation
/// retirement cannot publish metadata, but the original read can release its
/// own nonce under current session/owner authorization. A successor nonce is
/// never cleared. A lost session or process leaves the bounded lease to expire.
pub async fn release_read(pool: &PgPool, cycle: &Cycle) -> Result<(), DbError> {
    let mut tx = begin(pool, cycle.actor).await?;
    connector(&mut tx, cycle.actor, cycle.connector).await?;
    sqlx::query("UPDATE fvoci.zotero_connectors SET sync_id=NULL,sync_expires_at=NULL WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3 AND sync_id=$4")
        .bind(cycle.actor.workspace).bind(cycle.actor.user).bind(cycle.connector).bind(cycle.sync_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
/// Delays are durable; no sleeping daemon or invisible retry is created.
pub async fn finish_failed(
    pool: &PgPool,
    cycle: &Cycle,
    delay: i64,
    denied: bool,
) -> Result<(), DbError> {
    let mut tx = begin(pool, cycle.actor).await?;
    connector(&mut tx, cycle.actor, cycle.connector).await?;
    fence(&mut tx, cycle).await?;
    sqlx::query("UPDATE fvoci.zotero_connectors SET retry_at=CASE WHEN $2>0 THEN GREATEST(retry_at,now()+($2*interval '1 second')) ELSE retry_at END,state=CASE WHEN $3 THEN 'denied' ELSE state END,sync_id=NULL,sync_expires_at=NULL,updated_at=now() WHERE id=$1")
        .bind(cycle.connector).bind(delay).bind(denied).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn delay_next(pool: &PgPool, cycle: &Cycle, seconds: i64) -> Result<(), DbError> {
    let mut tx = begin(pool, cycle.actor).await?;
    connector(&mut tx, cycle.actor, cycle.connector).await?;
    fence(&mut tx, cycle).await?;
    sqlx::query(
        "UPDATE fvoci.zotero_connectors SET retry_at=GREATEST(retry_at,now()+($2*interval '1 second')) WHERE id=$1",
    )
    .bind(cycle.connector)
    .bind(seconds)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

type FinalPageData<'a> = (
    &'a Deleted,
    &'a BTreeMap<String, i64>,
    &'a BTreeMap<String, i64>,
);

/// `collections` is an independently validated complete mirror. Last item
/// batch, deletion/type-transition edges and completion C share one commit.
pub async fn commit_page(
    pool: &PgPool,
    cycle: &Cycle,
    remote_version: i64,
    collections: &[Collection],
    items: &[Item],
    final_data: Option<FinalPageData<'_>>,
) -> Result<(), DbError> {
    let actor = cycle.actor;
    let mut tx = begin(pool, actor).await?;
    connector(&mut tx, actor, cycle.connector).await?;
    fence(&mut tx, cycle).await?;
    // Retained tombstones count toward the readable mirror bound. Admit the
    // union before writing any collection, reference or completion watermark.
    let collection_keys: Vec<_> = collections.iter().map(|c| c.key.clone()).collect();
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_collections WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND NOT(collection_key=ANY($4))")
        .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(&collection_keys).fetch_one(&mut *tx).await?;
    if retained + collections.len() as i64 > zotero::KEY_MAX as i64 {
        return Err(ZoteroError::Limit.into());
    }
    for collection in collections {
        sqlx::query("INSERT INTO fvoci.zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(workspace_id,owner_user_id,connector_id,collection_key) DO UPDATE SET remote_version=EXCLUDED.remote_version,name=EXCLUDED.name,parent_key=EXCLUDED.parent_key,availability='available' WHERE fvoci.zotero_collections.remote_version<=EXCLUDED.remote_version")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(&collection.key).bind(collection.version).bind(&collection.name).bind(&collection.parent).execute(&mut *tx).await?;
    }
    for item in items {
        let old:Option<(Uuid,Option<Uuid>,i64,i64)>=sqlx::query_as("SELECT id,document_id,remote_version,local_version FROM fvoci.zotero_references WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND item_key=$4 FOR UPDATE")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(&item.key).fetch_optional(&mut *tx).await?;
        let before = old.map(|(_, _, _, v)| v);
        let id = if let Some((id, _, version, _)) = old {
            if version > item.version {
                return Err(ZoteroError::VersionChanged.into());
            }
            id
        } else {
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_references WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3").bind(actor.workspace).bind(actor.user).bind(cycle.connector).fetch_one(&mut *tx).await?;
            if count >= zotero::KEY_MAX as i64 {
                return Err(ZoteroError::Limit.into());
            }
            // This one-time ordinary title is authored locally thereafter.
            let document = create_wiki_document_tx(
                &mut tx,
                actor.workspace,
                actor.user,
                actor.session,
                CreateDocumentInput {
                    parent_id: None,
                    title: "Zotero reference",
                    icon: None,
                },
                None,
                None,
            )
            .await?
            .map_err(|_| DbError::NotFound)?
            .ok_or(DbError::NotFound)?;
            document.id
        };
        let metadata =
            serde_json::to_value(&item.bibliography).map_err(|_| ZoteroError::Invalid)?;
        let local_version:i64=sqlx::query_scalar("INSERT INTO fvoci.zotero_references(id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,bibliography,return_url,availability) VALUES($1,$2,$3,$4,$1,$5,$6,$7,$8,$9) ON CONFLICT(workspace_id,owner_user_id,connector_id,item_key) DO UPDATE SET remote_version=EXCLUDED.remote_version,bibliography=EXCLUDED.bibliography,return_url=EXCLUDED.return_url,availability=EXCLUDED.availability,local_version=fvoci.zotero_references.local_version+CASE WHEN (fvoci.zotero_references.bibliography,fvoci.zotero_references.availability,fvoci.zotero_references.return_url) IS DISTINCT FROM (EXCLUDED.bibliography,EXCLUDED.availability,EXCLUDED.return_url) THEN 1 ELSE 0 END RETURNING local_version")
            .bind(id).bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(&item.key).bind(item.version).bind(metadata).bind(&item.return_url).bind(if item.trashed{"trashed"}else{"available"}).fetch_one(&mut *tx).await?;
        if before.is_some_and(|v| local_version > v) && old.and_then(|(_, doc, _, _)| doc).is_some()
        {
            crate::db::documents::record_document_event_and_audit(
                &mut tx,
                actor.workspace,
                actor.user,
                "document.updated",
                id,
                serde_json::json!({"documentId":id}),
                None,
            )
            .await?;
        }
        sqlx::query("DELETE FROM fvoci.zotero_memberships WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND reference_id=$4")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(id).execute(&mut *tx).await?;
        for collection in &item.collections {
            let exists = collections.iter().any(|c| &c.key == collection);
            if !exists {
                return Err(ZoteroError::Invalid.into());
            }
            sqlx::query("INSERT INTO fvoci.zotero_memberships(workspace_id,owner_user_id,connector_id,reference_id,collection_key) VALUES($1,$2,$3,$4,$5)")
                .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(id).bind(collection).execute(&mut *tx).await?;
        }
    }
    if let Some((deleted, excluded, inventory)) = final_data {
        if cycle.since == 0 {
            let keys: Vec<_> = inventory.keys().cloned().collect();
            sqlx::query("UPDATE fvoci.zotero_references SET availability='deleted',local_version=local_version+CASE WHEN availability='deleted' THEN 0 ELSE 1 END WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND NOT(item_key=ANY($4))").bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(keys).execute(&mut *tx).await?;
        }
        for (key, version) in excluded {
            sqlx::query("UPDATE fvoci.zotero_references SET availability='excluded',remote_version=$5,local_version=local_version+1 WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND item_key=$4 AND remote_version<=$5")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(key).bind(version).execute(&mut *tx).await?;
        }
        for key in &deleted.items {
            sqlx::query("UPDATE fvoci.zotero_references SET availability='deleted',local_version=local_version+CASE WHEN availability='deleted' THEN 0 ELSE 1 END WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND item_key=$4")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(key).execute(&mut *tx).await?;
        }
        let collection_keys: Vec<_> = collections.iter().map(|c| c.key.clone()).collect();
        sqlx::query("UPDATE fvoci.zotero_collections SET availability='deleted' WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND NOT(collection_key=ANY($4))")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).bind(&collection_keys).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM fvoci.zotero_memberships m USING fvoci.zotero_references r,fvoci.zotero_collections c WHERE m.reference_id=r.id AND m.workspace_id=c.workspace_id AND m.owner_user_id=c.owner_user_id AND m.connector_id=c.connector_id AND m.collection_key=c.collection_key AND m.workspace_id=$1 AND m.owner_user_id=$2 AND m.connector_id=$3 AND (r.availability IN ('deleted','excluded') OR c.availability='deleted')")
            .bind(actor.workspace).bind(actor.user).bind(cycle.connector).execute(&mut *tx).await?;
        sqlx::query("UPDATE fvoci.zotero_connectors SET completed_version=$2,progress_version=NULL,committed_pages=0,reconciliation_required=false,sync_id=NULL,sync_expires_at=NULL,updated_at=now() WHERE id=$1")
            .bind(cycle.connector).bind(remote_version).execute(&mut *tx).await?;
    } else {
        sqlx::query("UPDATE fvoci.zotero_connectors SET progress_version=$2,committed_pages=committed_pages+1,updated_at=now() WHERE id=$1")
            .bind(cycle.connector).bind(remote_version).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn library(pool: &PgPool, actor: Actor, id: Uuid) -> Result<LibraryOutput, DbError> {
    let mut tx = begin(pool, actor).await?;
    let connector = connector(&mut tx, actor, id).await?;
    let rows=sqlx::query("SELECT r.*,d.number,COALESCE(p.key,'WIKI') AS document_prefix FROM fvoci.zotero_references r JOIN fvoci.documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id AND d.deleted_at IS NULL LEFT JOIN fvoci.projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id WHERE r.workspace_id=$1 AND r.owner_user_id=$2 AND r.connector_id=$3 ORDER BY r.item_key LIMIT 2001")
        .bind(actor.workspace).bind(actor.user).bind(id).fetch_all(&mut *tx).await?;
    if rows.len() > zotero::KEY_MAX {
        return Err(ZoteroError::Limit.into());
    }
    let mut references = Vec::new();
    for row in rows {
        let reference_id: Uuid = row.try_get("id")?;
        let document_id: Uuid = row.try_get("document_id")?;
        if !document_view_permission(&mut tx, actor.workspace, actor.user, document_id)
            .await?
            .at_least(ProjectPermission::View)
        {
            continue;
        }
        let bibliography = serde_json::from_value(row.try_get("bibliography")?)
            .map_err(|_| ZoteroError::Invalid)?;
        let collection_keys=sqlx::query_scalar("SELECT collection_key FROM fvoci.zotero_memberships WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND reference_id=$4 ORDER BY collection_key")
            .bind(actor.workspace).bind(actor.user).bind(id).bind(reference_id).fetch_all(&mut *tx).await?;
        let link_rows:Vec<(Option<Uuid>,Option<Uuid>,String)>=sqlx::query_as("SELECT document_id,task_id,anchor FROM fvoci.zotero_links WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 AND reference_id=$4 ORDER BY id LIMIT 101")
            .bind(actor.workspace).bind(actor.user).bind(id).bind(reference_id).fetch_all(&mut *tx).await?;
        if link_rows.len() > 100 {
            return Err(ZoteroError::Limit.into());
        }
        let mut links = Vec::new();
        for (document_id, task_id, anchor) in link_rows {
            let permission = if let Some(doc) = document_id {
                document_view_permission(&mut tx, actor.workspace, actor.user, doc).await?
            } else if let Some(task) = task_id {
                task_view_permission(&mut tx, actor.workspace, actor.user, task).await?
            } else {
                ProjectPermission::None
            };
            if permission.at_least(ProjectPermission::View) {
                let display_sql = if document_id.is_some() {
                    "SELECT COALESCE(p.key,'WIKI')||'-'||d.number FROM fvoci.documents d LEFT JOIN fvoci.projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id WHERE d.workspace_id=$1 AND d.id=$2 AND d.deleted_at IS NULL"
                } else {
                    "SELECT p.key||'-'||t.number FROM fvoci.tasks t JOIN fvoci.projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id WHERE t.workspace_id=$1 AND t.id=$2 AND t.deleted_at IS NULL"
                };
                let display_id: String = sqlx::query_scalar(display_sql)
                    .bind(actor.workspace)
                    .bind(document_id.or(task_id).ok_or(DbError::NotFound)?)
                    .fetch_one(&mut *tx)
                    .await?;
                links.push(LinkOutput {
                    display_id,
                    document_id,
                    task_id,
                    anchor: (!anchor.is_empty()).then_some(anchor),
                });
            }
        }
        references.push(ReferenceOutput {
            id: reference_id,
            document_display_id: format!(
                "{}-{}",
                row.try_get::<String, _>("document_prefix")?,
                row.try_get::<i32, _>("number")?
            ),
            connector_id: id,
            item_key: row.try_get("item_key")?,
            remote_version: row.try_get::<i64, _>("remote_version")?.to_string(),
            local_version: row.try_get::<i64, _>("local_version")?.to_string(),
            bibliography,
            return_url: row.try_get("return_url")?,
            availability: row.try_get("availability")?,
            collection_keys,
            links,
        });
    }
    let rows:Vec<(String,i64,String,Option<String>,String)>=sqlx::query_as("SELECT collection_key,remote_version,name,parent_key,availability FROM fvoci.zotero_collections WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3 ORDER BY collection_key LIMIT 2001")
        .bind(actor.workspace).bind(actor.user).bind(id).fetch_all(&mut *tx).await?;
    if rows.len() > zotero::KEY_MAX {
        return Err(ZoteroError::Limit.into());
    }
    let collections = rows
        .into_iter()
        .map(
            |(key, version, name, parent_key, availability)| ZoteroCollectionOutput {
                key,
                remote_version: version.to_string(),
                name,
                parent_key,
                availability,
            },
        )
        .collect();
    tx.commit().await?;
    Ok(LibraryOutput {
        connector,
        references,
        collections,
    })
}
pub async fn link(
    pool: &PgPool,
    actor: Actor,
    reference: Uuid,
    input: &LinkBody,
) -> Result<LibraryOutput, DbError> {
    if input.document_id.is_some() == input.task_id.is_some()
        || input
            .anchor
            .as_ref()
            .is_some_and(|a| a.is_empty() || a.chars().count() > 256)
    {
        return Err(ZoteroError::Invalid.into());
    }
    let expected = zotero::decimal(&input.expected_version)?;
    let mut tx = begin(pool, actor).await?;
    let row:Option<(Uuid,Option<Uuid>,i64)>=sqlx::query_as("SELECT connector_id,document_id,local_version FROM fvoci.zotero_references WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3 FOR UPDATE")
        .bind(actor.workspace).bind(actor.user).bind(reference).fetch_optional(&mut *tx).await?;
    let (connector_id, source, version) = row.ok_or(DbError::NotFound)?;
    let source = source.ok_or(DbError::NotFound)?;
    if !document_view_permission(&mut tx, actor.workspace, actor.user, source)
        .await?
        .at_least(ProjectPermission::View)
    {
        return Err(DbError::NotFound);
    }
    let permission = if let Some(doc) = input.document_id {
        link_permission(&mut tx, actor, doc, false).await?
    } else {
        link_permission(
            &mut tx,
            actor,
            input.task_id.ok_or(DbError::NotFound)?,
            true,
        )
        .await?
    };
    if !permission.at_least(ProjectPermission::Edit) {
        return Err(DbError::NotFound);
    }
    if version != expected {
        return Err(DbError::Conflict);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.zotero_links WHERE workspace_id=$1 AND reference_id=$2",
    )
    .bind(actor.workspace)
    .bind(reference)
    .fetch_one(&mut *tx)
    .await?;
    if count >= 100 {
        let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.zotero_links WHERE workspace_id=$1 AND reference_id=$2 AND document_id IS NOT DISTINCT FROM $3 AND task_id IS NOT DISTINCT FROM $4 AND anchor=$5)").bind(actor.workspace).bind(reference).bind(input.document_id).bind(input.task_id).bind(input.anchor.as_deref().unwrap_or("")).fetch_one(&mut *tx).await?;
        if !duplicate {
            return Err(ZoteroError::Limit.into());
        }
    }
    sqlx::query("INSERT INTO fvoci.zotero_links(id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor) VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(workspace_id,reference_id,document_id,task_id,anchor) DO NOTHING")
        .bind(Uuid::now_v7()).bind(actor.workspace).bind(actor.user).bind(connector_id).bind(reference).bind(input.document_id).bind(input.task_id).bind(input.anchor.as_deref().unwrap_or("")).execute(&mut *tx).await?;
    tx.commit().await?;
    library(pool, actor, connector_id).await
}
