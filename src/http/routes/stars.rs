//! Source `apps/server/src/domains/stars/routes.ts`: stars and recent items.
//! Auth `any`: sessions see everything; API tokens are narrowed to the content
//! kinds their read scopes grant (source `allowedContentKinds`).

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use chrono::SecondsFormat;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{
    OkResponse, RecentItemOutput, RecentListResponse, StarCreateBody, StarItemOutput,
    StarListResponse,
};
use crate::db::stars::{
    add_star, kind_str, list_recent, list_stars_backend, remove_star, RecentItem, StarDbError,
    StarItem, StarTarget, RECENT_DEFAULT_LIMIT, RECENT_MAX_LIMIT,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::routes::notifications::allowed_content_kinds;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentQuery {
    pub limit: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/stars",
            get(list_stars_route).post(create_star_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/stars/{id}",
            delete(remove_star_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/recent",
            get(list_recent_route),
        )
}

fn iso(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn star_output(item: StarItem) -> StarItemOutput {
    StarItemOutput {
        id: item.id.to_string(),
        r#type: kind_str(item.kind).to_string(),
        target_id: item.target_id.to_string(),
        title: item.title,
        project_id: item.project_id.map(|id| id.to_string()),
        number: item.number,
        created_at: iso(item.created_at),
    }
}

fn recent_output(item: RecentItem) -> RecentItemOutput {
    RecentItemOutput {
        r#type: kind_str(item.kind).to_string(),
        id: item.id.to_string(),
        title: item.title,
        project_id: item.project_id.map(|id| id.to_string()),
        number: item.number,
        updated_at: iso(item.updated_at),
    }
}

/// Non-member, dead credential and missing target all read as 404 (the Rust
/// API hides existence; the source documents 404 for these routes).
fn map_star_error(err: StarDbError) -> AppError {
    match err {
        StarDbError::NotFound | StarDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

/// Source `z.coerce.number().int().min(1).max(50).default(20)`.
fn parse_recent_limit(raw: Option<&str>) -> Result<i64, AppError> {
    let Some(raw) = raw else {
        return Ok(RECENT_DEFAULT_LIMIT);
    };
    let value: f64 = raw
        .trim()
        .parse()
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if raw.trim().is_empty()
        || !value.is_finite()
        || value.fract() != 0.0
        || !(1.0..=RECENT_MAX_LIMIT as f64).contains(&value)
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    Ok(value as i64)
}

/// Hyphenated form only, like the source `uuid` primitive.
pub(crate) fn parse_body_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    Uuid::parse_str(value).ok()
}

fn parse_star_target(body: &StarCreateBody) -> Result<StarTarget, AppError> {
    let id = parse_body_uuid(&body.id)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/id"))?;
    match body.r#type.as_str() {
        "document" => Ok(StarTarget::Document(id)),
        "task" => Ok(StarTarget::Task(id)),
        _ => Err(AppError::with_source(ProblemCode::InvalidInput, "/type")),
    }
}

async fn list_stars_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<StarListResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    let items = list_stars_backend(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(StarListResponse {
        items: items.into_iter().map(star_output).collect(),
    }))
}

async fn create_star_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<StarCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let target = parse_star_target(&body)?;
    let kinds = allowed_content_kinds(&auth);
    let item = add_star(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/stars.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        target,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok((StatusCode::CREATED, Json(star_output(item))).into_response())
}

async fn remove_star_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, star_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    remove_star(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/stars.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        star_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn list_recent_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<RecentQuery>, QueryRejection>,
) -> Result<Json<RecentListResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let limit = parse_recent_limit(query.limit.as_deref())?;
    let kinds = allowed_content_kinds(&auth);
    let items = list_recent(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/stars.rs")
            .map_err(internal)?,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        limit,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(RecentListResponse {
        items: items.into_iter().map(recent_output).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_limit_follows_source_coercion() {
        assert_eq!(parse_recent_limit(None).unwrap(), 20);
        assert_eq!(parse_recent_limit(Some("50")).unwrap(), 50);
        assert_eq!(parse_recent_limit(Some("1")).unwrap(), 1);
        assert!(parse_recent_limit(Some("0")).is_err());
        assert!(parse_recent_limit(Some("51")).is_err());
        assert!(parse_recent_limit(Some("2.5")).is_err());
        assert!(parse_recent_limit(Some("abc")).is_err());
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_list_access_http_tests {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    // Same maintained AppState construction as the selected import route
    // fixture, using the existing full-schema family fixture rather than a
    // new server/DB framework. These tests exercise the real leased handlers.
    fn state(f: &Fixture) -> AppState {
        AppState {
            realtime_mode:crate::config::RealtimeMode::On,native_engine:None,
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,
            rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.dir.join("storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,
            import_extractor_available:false,preview_extract:None,quota:Default::default(),
            mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }
    fn app(state: AppState) -> Router {
        router()
            .merge(crate::http::routes::groups::router())
            .merge(crate::http::routes::streams::router())
            .with_state(state)
    }
    async fn session(f: &Fixture) -> String {
        let token = crate::auth::token::new_token();
        sqlx::query("UPDATE sessions SET token_hash=?2 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .bind(token.hash)
            .execute(&f.pool)
            .await
            .unwrap();
        token.token
    }
    async fn response(app: Router, path: &str, token: Option<&str>, bearer: bool) -> Response {
        let mut req = axum::http::Request::builder()
            .uri(path)
            .header("origin", "http://localhost");
        if let Some(token) = token {
            req = if bearer {
                req.header("authorization", format!("Bearer {token}"))
            } else {
                req.header("cookie", format!("fvoci_session={token}"))
            };
        }
        app.oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn get(
        app: Router,
        path: &str,
        token: Option<&str>,
        bearer: bool,
    ) -> (StatusCode, Value) {
        let res = response(app, path, token, bearer).await;
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 16384).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn pat(f: &Fixture, scopes: &[&str]) -> String {
        let token = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'list test',?5)")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(token.hash).bind(serde_json::to_string(scopes).unwrap()).execute(&f.pool).await.unwrap();
        token.token
    }
    async fn close(f: Fixture) {
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
    async fn star(f: &Fixture, id: Uuid, target: Uuid, task: bool, at: i64) {
        sqlx::query("INSERT INTO stars(id,workspace_id,user_id,document_id,task_id,created_at) VALUES(?1,?2,?3,?4,?5,?6)")
            .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .bind((!task).then(||target.as_bytes().to_vec())).bind(task.then(||target.as_bytes().to_vec())).bind(at).execute(&f.pool).await.unwrap();
    }
    async fn project_targets(f: &Fixture) -> (Uuid, Uuid, Uuid) {
        let project = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        let task = Uuid::now_v7();
        let doc = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'LST','list project','private',?3)").bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'todo','todo','V')").bind(status.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,created_by,content_json) VALUES(?1,?2,?3,7,'작업 😀',?4,?5,'{\"type\":\"doc\"}')").bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,project_id,title,path,sort_key,number,created_by,content_json) VALUES(?1,?2,?3,'project document',?4,'V',8,?5,'{\"type\":\"doc\"}')").bind(doc.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(doc.simple().to_string()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        (project, doc, task)
    }

    #[tokio::test]
    async fn sqlite_http_stars_nonempty_dtos_order_current_acl_and_pat_kinds() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        let token = session(&f).await;
        let (project, doc, task) = project_targets(&f).await;
        let ids = [
            Uuid::from_u128(101),
            Uuid::from_u128(102),
            Uuid::from_u128(103),
        ];
        star(&f, ids[0], f.document, false, 1_000_000).await;
        star(&f, ids[1], doc, false, 2_000_000).await;
        star(&f, ids[2], task, true, 2_000_000).await;
        let app = app(state(&f));
        let path = format!("/api/v1/workspaces/{}/stars", f.workspace);
        let (status, body) = get(app.clone(), &path, Some(&token), false).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["items"],
            json!([
                {"id":ids[2],"type":"task","targetId":task,"title":"작업 😀","projectId":project,"number":7,"createdAt":"1970-01-01T00:00:02.000Z"},
                {"id":ids[1],"type":"document","targetId":doc,"title":"project document","projectId":project,"number":8,"createdAt":"1970-01-01T00:00:02.000Z"},
                {"id":ids[0],"type":"document","targetId":f.document,"title":"private wiki","projectId":null,"number":1,"createdAt":"1970-01-01T00:00:01.000Z"},
            ])
        );
        for (scopes, expected) in [
            (
                &["documents.read"][..],
                vec![ids[1].to_string(), ids[0].to_string()],
            ),
            (&["tasks.read"][..], vec![ids[2].to_string()]),
            (&["share.manage"][..], vec![]),
        ] {
            let pat = pat(&f, scopes).await;
            let (status, body) = get(app.clone(), &path, Some(&pat), true).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                body["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
                expected
            );
        }
        // Another live actor in the same tenant cannot see these user's stars.
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.other_user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other_token = crate::auth::token::new_token();
        sqlx::query("UPDATE sessions SET token_hash=?2 WHERE id=?1")
            .bind(f.other_credential.as_bytes().as_slice())
            .bind(other_token.hash)
            .execute(&f.pool)
            .await
            .unwrap();
        let other_star = Uuid::now_v7();
        sqlx::query("INSERT INTO stars(id,workspace_id,user_id,document_id) VALUES(?1,?2,?3,?4)")
            .bind(other_star.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.other_user.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (other_status, other_body) =
            get(app.clone(), &path, Some(&other_token.token), false).await;
        assert_eq!(other_status, StatusCode::OK);
        assert_eq!(other_body["items"].as_array().unwrap().len(), 1);
        assert_eq!(other_body["items"][0]["id"], other_star.to_string());
        assert_eq!(
            get(app.clone(), &path, Some(&token), false).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        for (table, target) in [
            ("documents", f.document),
            ("documents", doc),
            ("tasks", task),
        ] {
            sqlx::query(&format!("UPDATE {table} SET deleted_at=1 WHERE id=?1"))
                .bind(target.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let (status, visible) = get(app.clone(), &path, Some(&token), false).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                visible["items"].as_array().unwrap().len(),
                2,
                "deleted target must be hidden"
            );
            sqlx::query(&format!("UPDATE {table} SET deleted_at=NULL WHERE id=?1"))
                .bind(target.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE tasks SET archived_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(app.clone(), &path, Some(&token), false).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        sqlx::query("UPDATE tasks SET archived_at=NULL WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(app.clone(), &path, Some(&token), false).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        sqlx::query("UPDATE projects SET deleted_at=NULL WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM project_members WHERE project_id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, hidden) = get(app.clone(), &path, Some(&token), false).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(hidden["items"], json!([]));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM stars WHERE user_id=?1")
                .bind(f.user.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            3,
            "ACL hide does not delete saved stars"
        );
        drop(app);
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_http_groups_order_member_authority_pat_and_cross_tenant() {
        let f = Fixture::new().await;
        let token = session(&f).await;
        let ids = [Uuid::now_v7(), Uuid::now_v7()];
        for (id, name) in [(ids[1], "나중"), (ids[0], "먼저")] {
            sqlx::query("INSERT INTO groups(id,workspace_id,name,created_at,updated_at) VALUES(?1,?2,?3,1000000,2000000)").bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(name).execute(&f.pool).await.unwrap();
        }
        let app = app(state(&f));
        let path = format!("/api/v1/workspaces/{}/groups", f.workspace);
        let (status, body) = get(app.clone(), &path, Some(&token), false).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "guest member is an authorized list reader: {body}"
        );
        assert_eq!(
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![ids[0].to_string(), ids[1].to_string()]
        );
        assert_eq!(body["items"][0]["workspaceId"], f.workspace.to_string());
        assert_eq!(body["items"][0]["name"], "먼저");
        assert_eq!(body["items"][0]["createdAt"], "1970-01-01T00:00:01Z");
        assert_eq!(body["items"][0]["updatedAt"], "1970-01-01T00:00:02Z");
        let allowed = pat(&f, &["workspace.manage"]).await;
        assert_eq!(
            get(app.clone(), &path, Some(&allowed), true).await.0,
            StatusCode::OK
        );
        let denied = pat(&f, &["projects.read"]).await;
        assert_eq!(
            get(app.clone(), &path, Some(&denied), true).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(
                app.clone(),
                &format!("/api/v1/workspaces/{}/groups", f.other_workspace),
                Some(&token),
                false
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(app.clone(), &path, None, false).await.0,
            StatusCode::UNAUTHORIZED
        );
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(app.clone(), &path, Some(&token), false).await.0,
            StatusCode::NOT_FOUND
        );
        drop(app);
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_http_lists_recheck_current_credential_and_propagate_sql_fault() {
        let f = Fixture::new().await;
        let token = session(&f).await;
        let app = app(state(&f));
        for name in ["stars", "groups"] {
            let path = format!("/api/v1/workspaces/{}/{name}", f.workspace);
            assert_eq!(
                get(app.clone(), &path, Some(&token), false).await.0,
                StatusCode::OK
            );
            assert_eq!(
                get(
                    app.clone(),
                    &format!("/api/v1/workspaces/{}/{name}", f.other_workspace),
                    Some(&token),
                    false
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            // Route authorization has already accepted this real credential;
            // subsequent DB operation must recheck its now committed revocation.
            let headers = HeaderMap::from_iter([(
                axum::http::header::COOKIE,
                format!("fvoci_session={token}").parse().unwrap(),
            )]);
            let auth = require_request_auth(
                &state(&f),
                &headers,
                &CookieJar::from_headers(&headers),
                Access::Any,
                Some(f.workspace),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
                .bind(f.credential.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            if name == "stars" {
                assert!(matches!(
                    crate::db::stars::list_stars_backend(
                        &f.backend,
                        f.workspace,
                        auth.user_id,
                        auth.credential_id,
                        None
                    )
                    .await
                    .unwrap(),
                    Err(StarDbError::Forbidden)
                ));
            } else {
                assert!(matches!(
                    crate::db::groups::list_groups_backend(
                        &f.backend,
                        f.workspace,
                        auth.user_id,
                        auth.credential_id
                    )
                    .await
                    .unwrap(),
                    Err(crate::db::groups::GroupDbError::Forbidden)
                ));
            }
            assert_eq!(
                get(app.clone(), &path, Some(&token), false).await.0,
                StatusCode::UNAUTHORIZED
            );
            sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
                .bind(f.credential.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        for (column, actor) in [("suspended_at", f.user), ("deleted_at", f.user)] {
            sqlx::query(&format!("UPDATE users SET {column}=1 WHERE id=?1"))
                .bind(actor.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            for name in ["stars", "groups"] {
                assert_eq!(
                    get(
                        app.clone(),
                        &format!("/api/v1/workspaces/{}/{name}", f.workspace),
                        Some(&token),
                        false
                    )
                    .await
                    .0,
                    StatusCode::UNAUTHORIZED
                );
            }
            sqlx::query("UPDATE users SET suspended_at=NULL,deleted_at=NULL WHERE id=?1")
                .bind(actor.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for name in ["stars", "groups"] {
            assert_eq!(
                get(
                    app.clone(),
                    &format!("/api/v1/workspaces/{}/{name}", f.workspace),
                    Some(&token),
                    false
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
        }
        sqlx::query("UPDATE sessions SET expires_at=?2 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .bind(chrono::Utc::now().timestamp_micros() + 3_600_000_000i64)
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for name in ["stars", "groups"] {
            assert_eq!(
                get(
                    app.clone(),
                    &format!("/api/v1/workspaces/{}/{name}", f.workspace),
                    Some(&token),
                    false
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE groups")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(
                app.clone(),
                &format!("/api/v1/workspaces/{}/groups", f.workspace),
                Some(&token),
                false
            )
            .await
            .0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        sqlx::query("DROP TABLE stars")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            get(
                app.clone(),
                &format!("/api/v1/workspaces/{}/stars", f.workspace),
                Some(&token),
                false
            )
            .await
            .0,
            StatusCode::INTERNAL_SERVER_ERROR,
            "SQL fault cannot be hidden as empty list"
        );
        drop(app);
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_http_access_role_change_bystander_and_read_error_end_real_body() {
        let f = Fixture::new().await;
        let token = session(&f).await;
        let state = state(&f);
        let hub = state.streams.clone();
        let app = app(state);
        let path = format!("/api/v1/workspaces/{}/access-stream", f.workspace);
        let res = response(app.clone(), &path, Some(&token), false).await;
        assert_eq!(res.status(), StatusCode::OK);
        let mut body = res.into_body();
        for target in [f.actor, f.user] {
            let mut tx = f.backend.begin_write().await.unwrap();
            tx.operation().set_tenant(f.workspace).await.unwrap();
            tx.operation()
                .append_event(crate::db::identity::EventAppend {
                    id: Uuid::now_v7(),
                    workspace_id: Some(f.workspace),
                    actor_user_id: Some(f.actor),
                    verb: "workspace_member.role_changed".into(),
                    target_type: Some("workspace_member".into()),
                    target_id: Some(target),
                    payload: json!({}),
                })
                .await
                .unwrap();
            tx.commit().await.unwrap();
            if target == f.actor {
                assert!(
                    tokio::time::timeout(crate::streams::STREAM_POLL_INTERVAL * 2, body.frame())
                        .await
                        .is_err(),
                    "bystander event must leave actual body open"
                );
            }
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), body.frame())
                .await
                .unwrap()
                .is_none(),
            "current member role change must close actual body"
        );
        drop(body);
        assert_eq!(hub.active_count(), 0);
        let res = response(app.clone(), &path, Some(&token), false).await;
        assert_eq!(res.status(), StatusCode::OK);
        let mut body = res.into_body();
        sqlx::query("DROP TABLE event_sequence")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), body.frame())
                .await
                .unwrap()
                .is_none(),
            "read failure ends producer rather than emitting from stale cursor"
        );
        drop(body);
        assert_eq!(hub.active_count(), 0);
        drop(app);
        close(f).await;
    }

    #[tokio::test]
    async fn sqlite_http_access_stream_current_revocation_close_drop_and_guard() {
        let f = Fixture::new().await;
        let token = session(&f).await;
        let state = state(&f);
        let hub = state.streams.clone();
        let app = app(state);
        let path = format!("/api/v1/workspaces/{}/access-stream", f.workspace);
        assert_eq!(
            response(app.clone(), &path, None, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(hub.active_count(), 0);
        assert_eq!(
            response(
                app.clone(),
                &format!("/api/v1/workspaces/{}/access-stream", f.other_workspace),
                Some(&token),
                false
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(hub.active_count(), 0);
        let pat = pat(&f, &["workspace.manage"]).await;
        assert_eq!(
            response(app.clone(), &path, Some(&pat), true)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "access stream remains session-only"
        );
        assert_eq!(hub.active_count(), 0);
        let res = response(app.clone(), &path, Some(&token), false).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "text/event-stream");
        assert_eq!(
            res.headers()["cache-control"],
            "private, no-cache, no-transform"
        );
        assert_eq!(hub.active_count(), 1);
        let mut body = res.into_body();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), body.frame())
                .await
                .unwrap()
                .is_none(),
            "actual access producer must close after credential revocation"
        );
        drop(body);
        assert_eq!(hub.active_count(), 0);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        drop(response(app.clone(), &path, Some(&token), false).await);
        assert_eq!(hub.active_count(), 0, "body drop owns slot release");
        let guards = (0..crate::streams::MAX_CONCURRENT_STREAMS)
            .map(|_| hub.try_acquire().ok().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            response(app.clone(), &path, Some(&token), false)
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(guards);
        assert_eq!(hub.active_count(), 0);
        let res = response(app.clone(), &path, Some(&token), false).await;
        assert_eq!(res.status(), StatusCode::OK);
        let mut body = res.into_body();
        hub.begin_shutdown();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), body.frame())
                .await
                .unwrap()
                .is_none(),
            "server stop retires the actual access producer"
        );
        drop(body);
        assert_eq!(hub.active_count(), 0);
        let res = response(app.clone(), &path, Some(&token), false).await;
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(hub.active_count(), 0);
        drop(app);
        close(f).await;
    }
}
