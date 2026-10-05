use std::net::SocketAddr;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{LookupItemOutput, LookupListResponse};
use crate::auth::session::SessionUser;
use crate::db::lookup::{lookup_display_id_backend as lookup_display_id, LookupDbError};
use crate::error::{AppError, ProblemCode};
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;

const LOOKUP_IP_LIMIT: u32 = 120;
const LOOKUP_USER_LIMIT: u32 = 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookupQuery {
    pub project_id: Option<Uuid>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/lookup/{display_id}",
        get(lookup_display_id_route),
    )
}

async fn lookup_display_id_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, display_id)): Path<(Uuid, String)>,
    query: Result<Query<LookupQuery>, QueryRejection>,
) -> Result<Json<LookupListResponse>, AppError> {
    if display_id.trim().is_empty() || display_id.chars().count() > 64 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let Query(query) = query.map_err(AppError::from)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("lookup:ip:{ip}"), LOOKUP_IP_LIMIT)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("lookup:user:{actor_user_id}"), LOOKUP_USER_LIMIT)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = lookup_display_id(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        &display_id,
        query.project_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(LookupListResponse {
            items: items
                .into_iter()
                .map(|item| LookupItemOutput {
                    kind: item.kind,
                    id: item.id.to_string(),
                    display_id: item.display_id,
                    title: item.title,
                    project_id: item.project_id.map(|id| id.to_string()),
                })
                .collect(),
        })),
        Err(LookupDbError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::AuthenticationRequired))
        }
        Err(LookupDbError::NotFound) => Err(map_project_error(
            crate::db::projects::ProjectDbError::NotFound,
        )),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), AppError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn parse_user_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_lookup_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::lookup::selected_lookup_tests::{origin_task, project, session};
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(f: &Fixture) -> AppState {
        AppState {
            realtime_mode: crate::config::RealtimeMode::On,
            native_engine: None,
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("lookup-http-storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,import_extractor_available:false,preview_extract:None,quota:Default::default(),mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }

    async fn get(
        app: Router,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value, HeaderMap) {
        let mut builder = axum::http::Request::builder().method("GET").uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", format!("fvoci_session={cookie}"));
        }
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let mut request = builder.body(axum::body::Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
        ));
        let response = app.oneshot(request).await.unwrap();
        let code = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (code, serde_json::from_slice(&bytes).unwrap(), headers)
    }

    #[tokio::test]
    async fn wiki_aux_lookup_http_normal_auth_wire_validation_and_denials() {
        let f = Fixture::new().await;
        let (credential, cookie) = session(&f, f.user).await;
        let project = project(&f, credential).await;
        let command = Uuid::now_v7();
        let task = origin_task(&f, credential, project, command)
            .await
            .task_id();
        assert_eq!(
            origin_task(&f, credential, project, command).await,
            crate::db::task_origins::DocumentTaskOutcome::Replayed(task)
        );
        // Actual creator setup above; this lease tests the real HTTP lookup.
        // The separately owned normal POST adapter and detail/body route are
        // required integrated checks, not replaced by this metadata fixture.
        let app = router().with_state(state(&f));
        let base = format!("/api/v1/workspaces/{}/lookup", f.workspace);
        let task_path = format!("{base}/ORIGIN-2");
        let (code, body, _) = get(app.clone(), &task_path, Some(&cookie), None).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"items":[{"kind":"task","id":task.to_string(),"displayId":"ORIGIN-2","title":"실제 원본 작업 中 😀","projectId":project.to_string()}]})
        );
        let (code, wiki, _) =
            get(app.clone(), &format!("{base}/WIKI-1"), Some(&cookie), None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(
            wiki,
            json!({"items":[{"kind":"document","id":f.document.to_string(),"displayId":"WIKI-1","title":"S31","projectId":null}]})
        );
        for suffix in [
            "bad".to_owned(),
            format!("ORIGIN-2?projectId={}", Uuid::now_v7()),
            format!("WIKI-1?projectId={project}"),
        ] {
            let (code, body, _) = get(
                app.clone(),
                &format!("{base}/{suffix}"),
                Some(&cookie),
                None,
            )
            .await;
            assert_eq!(code, StatusCode::OK);
            assert_eq!(body, json!({"items":[]}));
        }
        for suffix in [
            "%20".to_owned(),
            "x".repeat(65),
            "WIKI-1?unknown=1".to_owned(),
            "WIKI-1?projectId=bad".to_owned(),
        ] {
            let (code, body, _) = get(
                app.clone(),
                &format!("{base}/{suffix}"),
                Some(&cookie),
                None,
            )
            .await;
            assert_eq!(code, StatusCode::BAD_REQUEST, "{suffix}: {body}");
            assert_eq!(body["code"], "invalid_input");
        }
        let (code, body, _) = get(app.clone(), &task_path, None, None).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "authentication_required");
        let pat = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Lookup any scope','[\"workspace.manage\"]')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
        let (code, pat_body, _) = get(app.clone(), &task_path, None, Some(&pat.token)).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(pat_body, body_for_task(task, project));
        let (code, body, _) = get(
            app.clone(),
            &task_path,
            Some("stale-cookie"),
            Some(&pat.token),
        )
        .await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "authentication_required");
        let other_workspace = Uuid::now_v7();
        let (code, body, _) = get(
            app.clone(),
            &format!("/api/v1/workspaces/{other_workspace}/lookup/ORIGIN-2"),
            None,
            Some(&pat.token),
        )
        .await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "not_found");
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other')")
            .bind(other.as_bytes().as_slice())
            .bind(format!("{other}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (_, other_cookie) = session(&f, other).await;
        let (code, body, _) = get(app.clone(), &task_path, Some(&other_cookie), None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, json!({"items":[]}));
        let (code, missing, _) = get(
            app.clone(),
            &format!("{base}/MISSING-2"),
            Some(&other_cookie),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(missing, body);
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (code, body, _) = get(app.clone(), &task_path, Some(&cookie), None).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "authentication_required");
        // Healthy independent credential still reads the same literal result.
        let app = router().with_state(state(&f));
        let (code, body, _) = get(app, &task_path, None, Some(&pat.token)).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body, body_for_task(task, project));
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    fn body_for_task(task: Uuid, project: Uuid) -> Value {
        json!({"items":[{"kind":"task","id":task.to_string(),"displayId":"ORIGIN-2","title":"실제 원본 작업 中 😀","projectId":project.to_string()}]})
    }

    #[tokio::test]
    async fn wiki_aux_lookup_http_user_and_ip_rate_limits_retry_after() {
        let f = Fixture::new().await;
        let (_, cookie) = session(&f, f.user).await;
        let app = router().with_state(state(&f));
        let path = format!("/api/v1/workspaces/{}/lookup/WIKI-1", f.workspace);
        for _ in 0..LOOKUP_USER_LIMIT {
            let (code, body, _) = get(app.clone(), &path, Some(&cookie), None).await;
            assert_eq!(code, StatusCode::OK, "{body}");
            assert_eq!(body["items"][0]["id"], f.document.to_string());
        }
        let (code, body, headers) = get(app, &path, Some(&cookie), None).await;
        assert_eq!(code, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limit_exceeded");
        assert_eq!(
            headers["retry-after"].to_str().unwrap(),
            body["params"]["retryAfter"].as_u64().unwrap().to_string()
        );
        let app = router().with_state(state(&f));
        for _ in 0..LOOKUP_IP_LIMIT {
            assert_eq!(
                get(app.clone(), &path, None, None).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        let (code, body, headers) = get(app, &path, None, None).await;
        assert_eq!(code, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["code"], "rate_limit_exceeded");
        assert_eq!(
            headers["retry-after"].to_str().unwrap(),
            body["params"]["retryAfter"].as_u64().unwrap().to_string()
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}
