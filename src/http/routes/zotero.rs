//! Session-owned personal routes; local POST never means a remote write.
use crate::api::zotero_dto::{
    ConnectBody, ConnectorListOutput, ConnectorOutput, LibraryOutput, LinkBody,
};
use crate::db::zotero::{self, Actor, Cycle, DbError};
use crate::error::{AppError, ProblemCode};
use crate::http::routes::tasks::TaskApiError;
use crate::http::{
    authz::{require_request_auth, Access},
    guard::check_origin,
    state::AppState,
};
use crate::integrations::zotero::{self as remote, ReadReply, ZoteroClient, ZoteroError};
use crate::integrations::Integrations;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn router(integrations: Arc<Integrations>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/zotero",
            get(list).post(connect),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}",
            get(read).delete(disconnect),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}/sync",
            post(sync),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/zotero/references/{reference_id}/links",
            post(link),
        )
        .layer(DefaultBodyLimit::max(8192))
        .layer(Extension(integrations))
}
fn error(error: DbError) -> TaskApiError {
    let code = match &error {
        DbError::NotFound => ProblemCode::NotFound,
        DbError::Conflict => ProblemCode::Conflict,
        DbError::Encryption => ProblemCode::EncryptionUnavailable,
        DbError::Remote(ZoteroError::Invalid | ZoteroError::Limit) => ProblemCode::InvalidInput,
        DbError::Remote(ZoteroError::Retired | ZoteroError::VersionChanged) => {
            ProblemCode::Conflict
        }
        _ => ProblemCode::IntegrationUnavailable,
    };
    let mut problem = AppError::from_code(code);
    if let DbError::Remote(remote) = error {
        let kind = match remote {
            ZoteroError::Denied => "denied",
            ZoteroError::Transient => "transient",
            ZoteroError::Delayed => "delayed",
            ZoteroError::Invalid => "invalid",
            ZoteroError::Limit => "limit",
            ZoteroError::VersionChanged => "versionChanged",
            ZoteroError::Retired => "retired",
        };
        problem.params = Some(serde_json::json!({"code":format!("zotero_{kind}")}));
    }
    // SQL causes and upstream error bodies may contain credential material.
    // They are deliberately neither formatted into a response nor traced here.
    problem.into()
}
async fn actor(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace: Uuid,
) -> Result<Actor, TaskApiError> {
    let auth = require_request_auth(state, headers, jar, Access::Session, Some(workspace)).await?;
    Ok(Actor {
        workspace,
        user: auth.user_id,
        session: auth.credential_id,
    })
}
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace): Path<Uuid>,
) -> Result<Json<ConnectorListOutput>, TaskApiError> {
    let actor = actor(&state, &headers, &jar, workspace).await?;
    Ok(Json(
        zotero::list(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            actor,
        )
        .await
        .map_err(error)?,
    ))
}
async fn connect(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace): Path<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<ConnectorOutput>), TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(&state, &headers, &jar, workspace).await?;
    let input: ConnectBody = remote::parse(&body).map_err(|e| error(e.into()))?;
    let keys = integrations
        .encryption_keys
        .as_deref()
        .ok_or_else(|| error(DbError::Encryption))?;
    Ok((
        StatusCode::CREATED,
        Json(
            zotero::connect(
                state
                    .auth
                    .db
                    .pool
                    .postgres("src/http/routes/zotero.rs")
                    .map_err(crate::http::routes::tasks::internal)?,
                actor,
                &input,
                keys,
            )
            .await
            .map_err(error)?,
        ),
    ))
}
async fn read(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<LibraryOutput>, TaskApiError> {
    let actor = actor(&state, &headers, &jar, workspace).await?;
    Ok(Json(
        zotero::library(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            actor,
            id,
        )
        .await
        .map_err(error)?,
    ))
}
async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ConnectorOutput>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(&state, &headers, &jar, workspace).await?;
    Ok(Json(
        zotero::disconnect(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            actor,
            id,
        )
        .await
        .map_err(error)?,
    ))
}
async fn link(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
    body: Bytes,
) -> Result<Json<LibraryOutput>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(&state, &headers, &jar, workspace).await?;
    let input: LinkBody = remote::parse(&body).map_err(|e| error(e.into()))?;
    Ok(Json(
        zotero::link(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            actor,
            id,
            &input,
        )
        .await
        .map_err(error)?,
    ))
}
async fn sync(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<LibraryOutput>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(&state, &headers, &jar, workspace).await?;
    let keys = integrations
        .encryption_keys
        .as_deref()
        .ok_or_else(|| error(DbError::Encryption))?;
    let cycle = zotero::start(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/zotero.rs")
            .map_err(crate::http::routes::tasks::internal)?,
        actor,
        id,
        keys,
    )
    .await
    .map_err(error)?;
    let result = tokio::time::timeout(
        remote::CYCLE_TIMEOUT,
        collect(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            &cycle,
            &integrations.zotero,
        ),
    )
    .await
    .unwrap_or_else(|_| Err(ZoteroError::Transient.into()));
    if let Err(ref failure) = result {
        // A retired credential cannot update state, retry time or the new cycle.
        let _ = zotero::finish_failed(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            &cycle,
            0,
            matches!(failure, DbError::Remote(ZoteroError::Denied)),
        )
        .await;
    }
    // The read has ended (including timeout/drop of its transport future).
    // Retiring a key kept this nonce so another library could not overlap it.
    let released = zotero::release_read(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/zotero.rs")
            .map_err(crate::http::routes::tasks::internal)?,
        &cycle,
    )
    .await;
    result.map_err(error)?;
    released.map_err(error)?;
    Ok(Json(
        zotero::library(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/zotero.rs")
                .map_err(crate::http::routes::tasks::internal)?,
            actor,
            id,
        )
        .await
        .map_err(error)?,
    ))
}

struct Reads<'a> {
    pool: &'a PgPool,
    cycle: &'a Cycle,
    client: &'a ZoteroClient,
    count: usize,
    delayed: bool,
}
impl Reads<'_> {
    async fn get(
        &mut self,
        resource: &str,
        params: &[(&str, String)],
        expected: Option<i64>,
    ) -> Result<ReadReply, DbError> {
        if self.delayed {
            return Err(ZoteroError::Delayed.into());
        }
        if self.count >= remote::REQUEST_MAX {
            return Err(ZoteroError::Limit.into());
        }
        self.count += 1;
        zotero::current(self.pool, self.cycle).await?;
        let reply = self
            .client
            .read(&self.cycle.library, &self.cycle.key, resource, params)
            .await?;
        // Link is an unconsumed navigation hint. Requests come only from the
        // fixed version/key plan; no link/header URL ever reaches transport.
        let delay = remote::delay(&reply)?;
        if delay > 0 {
            zotero::delay_next(self.pool, self.cycle, delay).await?;
            self.delayed = true;
        }
        remote::version(&reply, expected)?;
        if remote::body_contains_credential(&reply.body, &self.cycle.key)? {
            return Err(ZoteroError::Invalid.into());
        }
        Ok(reply)
    }
    async fn inventory(
        &mut self,
        resource: &str,
        params: &[(&str, String)],
        expected: Option<i64>,
    ) -> Result<(i64, BTreeMap<String, i64>), DbError> {
        let reply = self.get(resource, params, expected).await?;
        let version = remote::version(&reply, expected)?;
        let inventory = remote::versions(&reply.body, version)?;
        if let Some(total) = remote::header(&reply, "total-results")? {
            if remote::decimal(total)? != inventory.len() as i64 {
                return Err(ZoteroError::Invalid.into());
            }
        }
        Ok((version, inventory))
    }
}
/// Committed earlier pages remain durable if a later request/save fails. The
/// final batch and C commit together. A new attempt restarts from old C and
/// repeats idempotent keys, rather than treating P as a completed watermark.
pub async fn collect(pool: &PgPool, cycle: &Cycle, client: &ZoteroClient) -> Result<(), DbError> {
    let mut reads = Reads {
        pool,
        cycle,
        client,
        count: 0,
        delayed: false,
    };
    let (version, collection_keys) = reads
        .inventory("collections", &[("format", "versions".into())], None)
        .await?;
    if version < cycle.since {
        return Err(ZoteroError::VersionChanged.into());
    }
    let mut collections = Vec::new();
    for (key, object_version) in &collection_keys {
        let reply = reads
            .get(&format!("collections/{key}"), &[], Some(*object_version))
            .await?;
        collections.push(remote::collection(
            &reply.body,
            &cycle.library,
            &collection_keys,
            key,
        )?);
    }
    remote::collection_graph(&collections)?;
    let common = vec![
        ("format", "versions".to_owned()),
        ("since", cycle.since.to_string()),
        ("includeTrashed", "1".to_owned()),
    ];
    let (_, all_keys) = reads.inventory("items", &common, Some(version)).await?;
    let mut params = common.clone();
    params.push(("itemType", remote::bibliographic_types().join(" || ")));
    let (_, item_keys) = reads.inventory("items", &params, Some(version)).await?;
    let mut params = common.clone();
    params.push(("itemType", "note || attachment || annotation".into()));
    let (_, excluded) = reads.inventory("items", &params, Some(version)).await?;
    // A newly introduced type must not silently become an excluded note/file.
    let mut union = item_keys.clone();
    for (key, v) in &excluded {
        if union.insert(key.clone(), *v).is_some() {
            return Err(ZoteroError::Invalid.into());
        }
    }
    if union != all_keys {
        return Err(ZoteroError::Invalid.into());
    }
    let keys: Vec<_> = item_keys.iter().collect();
    let pages: Vec<_> = keys.chunks(remote::PAGE_SIZE).collect();
    let mut last_page = Vec::new();
    for (index, keys) in pages.iter().enumerate() {
        let requested: BTreeMap<_, _> = keys.iter().map(|(k, v)| ((*k).clone(), **v)).collect();
        let key_list = requested.keys().cloned().collect::<Vec<_>>().join(",");
        let reply = reads
            .get(
                "items",
                &[
                    ("format", "json".into()),
                    ("itemKey", key_list),
                    ("includeTrashed", "1".into()),
                    ("limit", remote::PAGE_SIZE.to_string()),
                ],
                Some(version),
            )
            .await?;
        if let Some(total) = remote::header(&reply, "total-results")? {
            if remote::decimal(total)? != requested.len() as i64 {
                return Err(ZoteroError::Invalid.into());
            }
        }
        let items = remote::items(&reply.body, &cycle.library, &requested)?;
        if index + 1 == pages.len() {
            last_page = items;
        } else {
            zotero::commit_page(pool, cycle, version, &collections, &items, None).await?;
        }
    }
    let reply = reads
        .get(
            "deleted",
            &[("since", cycle.since.to_string())],
            Some(version),
        )
        .await?;
    let deleted: remote::Deleted = remote::parse(&reply.body)?;
    deleted.validate()?;
    if deleted.items.iter().any(|key| item_keys.contains_key(key))
        || deleted
            .collections
            .iter()
            .any(|key| collection_keys.contains_key(key))
    {
        return Err(ZoteroError::Invalid.into());
    }
    // Detect library changes during single-object collection reads and gaps.
    let (_, final_collections) = reads
        .inventory(
            "collections",
            &[("format", "versions".into())],
            Some(version),
        )
        .await?;
    if final_collections != collection_keys {
        return Err(ZoteroError::VersionChanged.into());
    }
    zotero::commit_page(
        pool,
        cycle,
        version,
        &collections,
        &last_page,
        Some((&deleted, &excluded, &all_keys)),
    )
    .await
}
