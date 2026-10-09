use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    CreateWorkspaceBody, DeleteWorkspaceBody, MemberResponse, MemberRoleBody, MembersResponse,
    OkResponse, PatchWorkspaceBody, WorkspaceListItemResponse, WorkspaceListResponse,
    WorkspaceMetaResponse,
};
use crate::auth::session::SessionUser;
use crate::db::workspace::{WorkspaceDbError, WorkspaceRole};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::{normalize_slug, validate_given_name};

mod events;
mod export;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/workspaces", get(list_my_workspaces))
        .route("/api/v1/me/personal-workspace", post(personal_workspace))
        .route("/api/v1/workspaces", post(create_workspace))
        .route(
            "/api/v1/workspaces/{workspace_id}",
            get(get_workspace)
                .patch(patch_workspace)
                .delete(delete_workspace),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            get(list_members),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            patch(patch_member).delete(remove_member),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/export",
            get(export::workspace_export),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/events",
            get(events::list_workspace_events_route),
        )
}

async fn list_my_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WorkspaceListResponse>, AppError> {
    let (_user, user_id, _) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let listed =
        crate::db::workspace::list_workspaces_for_user_backend(&state.auth.db.pool, user_id)
            .await
            .map_err(internal)?;
    let items = listed
        .into_iter()
        .map(|w| WorkspaceListItemResponse {
            id: w.id.to_string(),
            name: w.name,
            slug: w.slug,
            role: w.role.as_str().to_string(),
            kind: w.kind,
            document_count: w.document_count,
            assigned_count: w.assigned_count,
        })
        .collect();
    Ok(Json(WorkspaceListResponse { items }))
}

async fn get_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
    let result = crate::db::workspace::get_workspace_meta_backend(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<MembersResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result = crate::db::workspace::list_members_backend(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(members) => Ok(Json(MembersResponse {
            items: members
                .into_iter()
                .map(|member| MemberResponse {
                    user_id: member.user_id.to_string(),
                    email: member.email,
                    given_name: member.given_name,
                    family_name: member.family_name,
                    role: member.role.as_str().to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn patch_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<PatchWorkspaceBody>, JsonRejection>,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.name.is_none() {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let name = body.name.as_ref().unwrap();
    validate_given_name(name.trim())?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::update_workspace_meta(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/workspaces.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        session_id,
        name.trim(),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn create_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<CreateWorkspaceBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    validate_given_name(body.name.trim())?;
    let slug = normalize_slug(&body.slug)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::create_workspace_as_instance_admin(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/workspaces.rs")
            .map_err(internal)?,
        &state.auth.db.license,
        user_id,
        session_id,
        body.name.trim(),
        &slug,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok((StatusCode::CREATED, Json(meta_response(meta))).into_response()),
        Err(WorkspaceDbError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::InsufficientPermissions))
        }
        Err(WorkspaceDbError::SlugTaken) => Err(AppError::from_code(ProblemCode::SlugTaken)),
        Err(err) => Err(map_workspace_error(err, true)),
    }
}

async fn personal_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::ensure_personal_workspace_backend(
        &state.auth.db.pool,
        &state.auth.db.license,
        user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn patch_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<MemberRoleBody>, JsonRejection>,
) -> Result<Json<MemberResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let next_role = WorkspaceRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::set_member_role(
        &state.auth.db,
        workspace_id,
        actor_user_id,
        session_id,
        target_user_id,
        next_role,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(member) => Ok(Json(MemberResponse {
            user_id: member.user_id.to_string(),
            email: member.email,
            given_name: member.given_name,
            family_name: member.family_name,
            role: member.role.as_str().to_string(),
        })),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn remove_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::remove_member_backend(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        target_user_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn delete_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<DeleteWorkspaceBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let confirm_slug = normalize_slug(&body.confirm_slug)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::trash_workspace(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/workspaces.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        session_id,
        &confirm_slug,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(WorkspaceDbError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::InsufficientPermissions))
        }
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

fn meta_response(meta: crate::db::workspace::WorkspaceMeta) -> WorkspaceMetaResponse {
    WorkspaceMetaResponse {
        id: meta.id.to_string(),
        name: meta.name,
        slug: meta.slug,
    }
}

fn map_workspace_error(err: WorkspaceDbError, create_route: bool) -> AppError {
    match err {
        WorkspaceDbError::NotFound => AppError::from_code(ProblemCode::NotFound),
        WorkspaceDbError::Forbidden if create_route => {
            AppError::from_code(ProblemCode::InsufficientPermissions)
        }
        WorkspaceDbError::Forbidden => AppError::from_code(ProblemCode::NotFound),
        WorkspaceDbError::PersonalImmutable => {
            AppError::from_code(ProblemCode::PersonalWorkspaceImmutable)
        }
        WorkspaceDbError::LastOwner => AppError::from_code(ProblemCode::WorkspaceLastOwnerRequired),
        WorkspaceDbError::SelfChange => {
            AppError::from_code(ProblemCode::WorkspaceMemberSelfChangeForbidden)
        }
        WorkspaceDbError::RoleCap => AppError::from_code(ProblemCode::CannotManageRoleAboveOwn),
        WorkspaceDbError::SlugTaken => AppError::from_code(ProblemCode::SlugTaken),
        WorkspaceDbError::LastProjectLead => AppError::from_code(ProblemCode::Conflict),
        WorkspaceDbError::SeatLimit => AppError::from_code(ProblemCode::LimitSeats),
        WorkspaceDbError::GuestLimit => AppError::from_code(ProblemCode::LimitGuests),
        WorkspaceDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
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
mod selected_personal_bootstrap_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::workspace::selected_personal_workspace_tests::{
        assert_publication, fixture, foreign_keys, snapshot,
    };
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(f: &Fixture) -> Router {
        router().with_state(AppState {
            realtime_mode:crate::config::RealtimeMode::Off,native_engine:None,
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,
            rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,
            import_extractor_available:false,preview_extract:None,quota:Default::default(),
            mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        })
    }
    async fn cookie(f: &Fixture, credential: Uuid) -> String {
        let token = crate::auth::token::new_token();
        sqlx::query("UPDATE sessions SET token_hash=?1 WHERE id=?2")
            .bind(token.hash)
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        token.token
    }
    async fn post(
        app: Router,
        token: Option<&str>,
        bearer: bool,
        origin: &str,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/me/personal-workspace")
            .extension(ConnectInfo(
                "203.0.113.70:42424".parse::<SocketAddr>().unwrap(),
            ))
            .header("origin", origin);
        if let Some(token) = token {
            request = if bearer {
                request.header("authorization", format!("Bearer {token}"))
            } else {
                request.header("cookie", format!("fvoci_session={token}"))
            };
        }
        let response = app
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    mod member_removal {
        use super::*;
        use crate::db::workspace::selected_member_removal_tests::{
            assert_publication as assert_removal_publication, fixture as removal_fixture,
            snapshot as removal_snapshot,
        };

        async fn delete(
            app: Router,
            workspace: Uuid,
            target: Uuid,
            token: Option<&str>,
            bearer: bool,
            origin: &str,
        ) -> (StatusCode, Value) {
            let mut request = axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/workspaces/{workspace}/members/{target}"))
                .extension(ConnectInfo(
                    "203.0.113.71:42424".parse::<SocketAddr>().unwrap(),
                ))
                .header("origin", origin);
            if let Some(token) = token {
                request = if bearer {
                    request.header("authorization", format!("Bearer {token}"))
                } else {
                    request.header("cookie", format!("fvoci_session={token}"))
                };
            }
            let response = app
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            (status, serde_json::from_slice(&bytes).unwrap())
        }

        #[tokio::test]
        async fn sqlite_http_member_delete_cookie_origin_pat_tenant_and_literal_success() {
            let (f, credential, target, peer_credential) = removal_fixture().await;
            let token = cookie(&f, credential).await;
            let peer = cookie(&f, peer_credential).await;
            let app = app(&f);
            let pat = crate::auth::token::new_token();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Removal test','[\"workspace.manage\"]')")
                .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
            for (auth, bearer, origin, workspace, status) in [
                (
                    None,
                    false,
                    "http://localhost",
                    f.workspace,
                    StatusCode::UNAUTHORIZED,
                ),
                (
                    Some(token.as_str()),
                    false,
                    "http://foreign.test",
                    f.workspace,
                    StatusCode::FORBIDDEN,
                ),
                (
                    Some(pat.token.as_str()),
                    true,
                    "http://localhost",
                    f.workspace,
                    StatusCode::NOT_FOUND,
                ),
                (
                    Some(peer.as_str()),
                    false,
                    "http://localhost",
                    f.workspace,
                    StatusCode::NOT_FOUND,
                ),
                (
                    Some(token.as_str()),
                    false,
                    "http://localhost",
                    Uuid::now_v7(),
                    StatusCode::NOT_FOUND,
                ),
            ] {
                let before = removal_snapshot(&f).await;
                let (actual, body) =
                    delete(app.clone(), workspace, target, auth, bearer, origin).await;
                assert_eq!(actual, status, "{body}");
                assert!(body.get("ok").is_none());
                assert!(body.get("userId").is_none());
                assert_eq!(removal_snapshot(&f).await, before);
            }
            let (status, body) = delete(
                app.clone(),
                f.workspace,
                target,
                Some(&token),
                false,
                "http://localhost",
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, json!({"ok":true}));
            assert_removal_publication(&f, target, 0, 0).await;
            let before = removal_snapshot(&f).await;
            assert_eq!(
                delete(
                    app.clone(),
                    f.workspace,
                    target,
                    Some(&token),
                    false,
                    "http://localhost"
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            assert_eq!(removal_snapshot(&f).await, before);
            foreign_keys(&f).await;
            drop(app);
            f.close().await;
        }

        #[tokio::test]
        async fn sqlite_http_member_delete_audit_rollback_private_lead_refusal_and_healthy_progress(
        ) {
            let (f, credential, target, _) = removal_fixture().await;
            let token = cookie(&f, credential).await;
            let app = app(&f);
            let project =
                crate::db::workspace::selected_member_removal_tests::project(&f, target, "private")
                    .await;
            let before = removal_snapshot(&f).await;
            assert_eq!(
                delete(
                    app.clone(),
                    f.workspace,
                    target,
                    Some(&token),
                    false,
                    "http://localhost"
                )
                .await
                .0,
                StatusCode::CONFLICT
            );
            assert_eq!(removal_snapshot(&f).await, before);
            sqlx::query("UPDATE projects SET visibility='workspace' WHERE id=?1")
                .bind(project.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let before = removal_snapshot(&f).await;
            sqlx::query("CREATE TRIGGER http_remove_refuse BEFORE INSERT ON audit_log WHEN NEW.verb='workspace_member.removed' BEGIN SELECT RAISE(ABORT,'HTTP removal audit refused'); END").execute(&f.pool).await.unwrap();
            let (status, body) = delete(
                app.clone(),
                f.workspace,
                target,
                Some(&token),
                false,
                "http://localhost",
            )
            .await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
            assert!(body.get("ok").is_none());
            assert_eq!(removal_snapshot(&f).await, before);
            sqlx::query("DROP TRIGGER http_remove_refuse")
                .execute(&f.pool)
                .await
                .unwrap();
            let (status, body) = delete(
                app.clone(),
                f.workspace,
                target,
                Some(&token),
                false,
                "http://localhost",
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body, json!({"ok":true}));
            assert_removal_publication(&f, target, 0, 0).await;
            foreign_keys(&f).await;
            drop(app);
            f.close().await;
        }
    }

    #[tokio::test]
    async fn sqlite_http_personal_bootstrap_cookie_origin_session_only_and_stable_replay() {
        let (f, credential) = fixture().await;
        let token = cookie(&f, credential).await;
        let app = app(&f);
        let pat = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Bootstrap test','[\"workspace.manage\"]')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
        for (auth, bearer, origin, expected) in [
            (None, false, "http://localhost", StatusCode::UNAUTHORIZED),
            (
                Some(token.as_str()),
                false,
                "http://foreign.test",
                StatusCode::FORBIDDEN,
            ),
            (
                Some(pat.token.as_str()),
                true,
                "http://localhost",
                StatusCode::NOT_FOUND,
            ),
        ] {
            let before = snapshot(&f).await;
            let (status, body) = post(app.clone(), auth, bearer, origin).await;
            assert_eq!(status, expected, "{body}");
            assert!(body.get("id").is_none());
            assert!(body.get("slug").is_none());
            assert_eq!(snapshot(&f).await, before);
        }
        let (status, body) = post(app.clone(), Some(&token), false, "http://localhost").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
        let expected_slug = crate::db::workspace::personal_workspace_slug(f.user);
        assert_eq!(
            body,
            json!({"id":id.to_string(),"name":"Personal","slug":expected_slug})
        );
        assert_publication(
            &f,
            &crate::db::workspace::WorkspaceMeta {
                id,
                name: "Personal".into(),
                slug: expected_slug,
            },
            Some("203.0.113.70"),
        )
        .await;
        let before = snapshot(&f).await;
        let (a, b) = tokio::join!(
            post(app.clone(), Some(&token), false, "http://localhost"),
            post(app.clone(), Some(&token), false, "http://localhost")
        );
        assert_eq!(a, (StatusCode::OK, body.clone()));
        assert_eq!(b, (StatusCode::OK, body));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert_eq!(
            post(app.clone(), Some(&token), false, "http://localhost")
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(snapshot(&f).await, before);
        foreign_keys(&f).await;
        drop(app);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_personal_bootstrap_seat_limit_and_publication_failure_then_healthy_retry()
    {
        let (f, credential) = fixture().await;
        let token = cookie(&f, credential).await;
        let app = app(&f);
        sqlx::query("UPDATE memberships SET role='guest' WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut seats = Vec::new();
        for n in 0..10 {
            let id = Uuid::now_v7();
            seats.push(id);
            sqlx::query(
                "INSERT INTO users(id,email,given_name,is_instance_admin) VALUES(?1,?2,'Seat',1)",
            )
            .bind(id.as_bytes().as_slice())
            .bind(format!("httpseat{n}@quota.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let before = snapshot(&f).await;
        let (status, body) = post(app.clone(), Some(&token), false, "http://localhost").await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
        assert_eq!(body["code"], "limit.seats");
        assert!(body.get("id").is_none());
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE users SET anonymized_at=1 WHERE id=?1")
            .bind(seats[0].as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        sqlx::query("CREATE TRIGGER personal_http_refuse BEFORE INSERT ON audit_log WHEN NEW.verb='workspace.personal_created' BEGIN SELECT RAISE(ABORT,'personal HTTP audit refused'); END")
            .execute(&f.pool).await.unwrap();
        let (status, body) = post(app.clone(), Some(&token), false, "http://localhost").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(body.get("id").is_none());
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("DROP TRIGGER personal_http_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, body) = post(app.clone(), Some(&token), false, "http://localhost").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let meta = crate::db::workspace::WorkspaceMeta {
            id: Uuid::parse_str(body["id"].as_str().unwrap()).unwrap(),
            name: body["name"].as_str().unwrap().into(),
            slug: body["slug"].as_str().unwrap().into(),
        };
        assert_publication(&f, &meta, Some("203.0.113.70")).await;
        foreign_keys(&f).await;
        drop(app);
        f.close().await;
    }
}
