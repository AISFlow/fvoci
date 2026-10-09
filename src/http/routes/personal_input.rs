//! Intent capture is session-owned, private by construction and atomic in Rust.
use crate::api::personal_input_dto::{PersonalInputBody, PersonalInputIntent, PersonalInputOutput};
use crate::db::personal_input::{create_personal_input_backend, PersonalInputDbError};
use crate::error::{AppError, ProblemCode};
use crate::http::routes::{
    task_body::map_origin_error,
    tasks::{activity_channel, internal, map_task_db_error, TaskApiError},
};
use crate::http::{
    authz::{require_request_auth, Access},
    guard::check_origin,
    rate_limit::peer_ip,
    state::AppState,
};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use axum_extra::extract::CookieJar;
use std::net::SocketAddr;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/personal-input",
        post(create),
    )
}
fn valid(input: &PersonalInputBody) -> bool {
    crate::db::documents::title_is_valid(input.title.trim())
        && (input.intent == PersonalInputIntent::Task
            || (input.source.is_none() && input.project_id.is_none()))
        && input.source.as_ref().is_none_or(|source| {
            source.anchor.as_ref().is_none_or(|anchor| {
                !anchor.is_empty()
                    && anchor.chars().count()
                        <= crate::db::task_origins::TASK_ORIGIN_ANCHOR_MAX_CHARS
            })
        })
}
async fn create(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Bytes,
) -> Result<(StatusCode, Json<PersonalInputOutput>), TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let input: PersonalInputBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if !valid(&input) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    let ip = peer_ip(peer.ip());
    let output = create_personal_input_backend(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        &input,
        Some(&ip),
        activity_channel(&headers),
    )
    .await
    .map_err(internal)?
    .map_err(|err| match err {
        PersonalInputDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        PersonalInputDbError::Forbidden => {
            AppError::from_code(ProblemCode::InsufficientPermissions).into()
        }
        PersonalInputDbError::RequestMismatch => {
            map_origin_error(crate::db::task_origins::TaskOriginDbError::RequestMismatch)
        }
        PersonalInputDbError::Project(err) => map_task_db_error(err),
        PersonalInputDbError::Origin(err) => map_origin_error(err),
    })?;
    Ok((StatusCode::CREATED, Json(output)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_note_task_options_and_blank_input() {
        let mut input: PersonalInputBody = serde_json::from_value(
            serde_json::json!({"requestId":Uuid::nil(),"intent":"note","title":"한글 🙂"}),
        )
        .unwrap();
        assert!(valid(&input));
        input.project_id = Some(Uuid::nil());
        assert!(!valid(&input));
        input.intent = PersonalInputIntent::Task;
        assert!(valid(&input));
        input.title = " ".into();
        assert!(!valid(&input));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_personal_input_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::personal_input::selected_personal_input_tests::{input, setup, snapshot};
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
    async fn token(f: &Fixture, credential: Uuid) -> String {
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
        workspace: Uuid,
        token: Option<&str>,
        bearer: bool,
        origin: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/v1/workspaces/{workspace}/personal-input"))
            .extension(ConnectInfo(
                "127.0.0.1:31245".parse::<SocketAddr>().unwrap(),
            ))
            .header("origin", origin)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = if bearer {
                request.header("authorization", format!("Bearer {token}"))
            } else {
                request.header("cookie", format!("fvoci_session={token}"))
            };
        }
        let response = app
            .oneshot(
                request
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    fn problem(status: StatusCode, body: &Value, code: ProblemCode) {
        let (title, wire_code) = match code {
            ProblemCode::AuthenticationRequired => {
                ("authentication required", "authentication_required")
            }
            ProblemCode::OriginMismatch => ("origin mismatch", "origin_mismatch"),
            ProblemCode::InvalidInput => ("invalid input", "invalid_input"),
            ProblemCode::NotFound => ("not found", "not_found"),
            ProblemCode::InternalError => ("internal error", "internal_error"),
            _ => panic!("unexpected personal-input problem oracle"),
        };
        assert_eq!(
            body,
            &json!({"type":"about:blank","title":title,"status":status.as_u16(),"code":wire_code})
        );
    }

    #[tokio::test]
    async fn sqlite_http_note_quick_task_stable_replay_and_changed_command() {
        let (f, credential) = setup().await;
        let token = token(&f, credential).await;
        let app = app(&f);
        for intent in [
            PersonalInputIntent::Note,
            PersonalInputIntent::Quick,
            PersonalInputIntent::Task,
        ] {
            let request = input(intent);
            let body = serde_json::to_value(&request).unwrap();
            let (status, created) = post(
                app.clone(),
                f.workspace,
                Some(&token),
                false,
                "http://localhost",
                body.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(created["replayed"], false);
            let document = Uuid::parse_str(created["documentId"].as_str().unwrap()).unwrap();
            let title: String = sqlx::query_scalar("SELECT title FROM documents WHERE id=?1")
                .bind(document.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(title, request.title.trim());
            assert!(created["documentDisplayId"]
                .as_str()
                .unwrap()
                .starts_with("WIKI-"));
            if intent == PersonalInputIntent::Task {
                assert!(created["taskId"].is_string());
                assert_eq!(created["taskDisplayId"], "INBOX-2");
                assert!(created["projectId"].is_string());
            } else {
                assert!(created["taskId"].is_null());
                assert!(created["taskDisplayId"].is_null());
                assert!(created["projectId"].is_null());
            }
            let after = snapshot(&f).await;
            let (status, mut replay) = post(
                app.clone(),
                f.workspace,
                Some(&token),
                false,
                "http://localhost",
                body.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(replay["replayed"], true);
            replay["replayed"] = json!(false);
            assert_eq!(replay, created);
            assert_eq!(snapshot(&f).await, after);
            let mut changed = body;
            changed["title"] = json!("different");
            let (status, problem_body) = post(
                app.clone(),
                f.workspace,
                Some(&token),
                false,
                "http://localhost",
                changed,
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(
                problem_body,
                json!({"type":"about:blank","title":"document version mismatch","status":409,"code":"document_version_mismatch"})
            );
            assert_eq!(snapshot(&f).await, after);
        }
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_session_origin_input_and_personal_owner_denials() {
        let (f, credential) = setup().await;
        let token = token(&f, credential).await;
        let app = app(&f);
        let request = input(PersonalInputIntent::Note);
        let body = serde_json::to_value(&request).unwrap();
        let before = snapshot(&f).await;
        let (status, result) = post(
            app.clone(),
            f.workspace,
            None,
            false,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        problem(status, &result, ProblemCode::AuthenticationRequired);
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&token),
            false,
            "http://foreign.test",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        problem(status, &result, ProblemCode::OriginMismatch);
        let mut invalid = body.clone();
        invalid["projectId"] = json!(Uuid::nil());
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            invalid,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        problem(status, &result, ProblemCode::InvalidInput);
        let pat = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'personal-input test',?5)")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(pat.hash).bind(json!(["documents.read","documents.write"]).to_string()).execute(&f.pool).await.unwrap();
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&pat.token),
            true,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        problem(status, &result, ProblemCode::NotFound);
        let (status, result) = post(
            app.clone(),
            Uuid::now_v7(),
            Some(&token),
            false,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        problem(status, &result, ProblemCode::NotFound);
        sqlx::query("UPDATE users SET personal_workspace_id=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        problem(status, &result, ProblemCode::NotFound);
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, result) = post(
            app,
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        problem(status, &result, ProblemCode::AuthenticationRequired);
        assert_eq!(snapshot(&f).await, before);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_http_receipt_failure_is_private_500_atomic_then_same_command_retry() {
        let (f, credential) = setup().await;
        let token = token(&f, credential).await;
        let app = app(&f);
        let request = input(PersonalInputIntent::Task);
        let body = serde_json::to_value(&request).unwrap();
        let before = snapshot(&f).await;
        sqlx::query("CREATE TABLE personal_http_fk_probe(id BLOB REFERENCES documents(id)) STRICT")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER personal_http_refuse AFTER INSERT ON personal_input_commands BEGIN INSERT INTO personal_http_fk_probe(id) VALUES(zeroblob(16)); END").execute(&f.pool).await.unwrap();
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        problem(status, &result, ProblemCode::InternalError);
        let serialized = result.to_string();
        for private in [
            token.as_str(),
            request.title.as_str(),
            "personal_http_fk_probe",
            "personal_input_commands",
        ] {
            assert!(!serialized.contains(private));
        }
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("DROP TRIGGER personal_http_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, result) = post(
            app.clone(),
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(result["taskDisplayId"], "INBOX-2");
        assert_eq!(result["documentDisplayId"], "WIKI-2");
        assert_eq!(result["replayed"], false);
        let after = snapshot(&f).await;
        let (status, replay) = post(
            app,
            f.workspace,
            Some(&token),
            false,
            "http://localhost",
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["taskId"], result["taskId"]);
        assert_eq!(replay["documentId"], result["documentId"]);
        assert_eq!(snapshot(&f).await, after);
        f.close().await;
    }
}
