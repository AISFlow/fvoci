use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::ImportJobResponse;
use crate::collab::seed::SeedEngine;
use crate::db::import_jobs::{
    create_async_import_job_backend as create_async_import_job,
    create_sync_import_job_backend as create_sync_import_job,
    get_import_job_backend as get_import_job, ImportDbError, ImportSource, NewAsyncImport,
    IMPORT_HTTP_MAX_BYTES,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::state::AppState;
use crate::import_job::{
    office_format_supported, run_markdown_zip_import_backend as run_markdown_zip_import,
    SyncImportError,
};

/// JSON body cap: a 64 MiB file as base64 (4/3) plus room for the other fields.
pub const IMPORT_BODY_MAX_BYTES: usize = IMPORT_HTTP_MAX_BYTES / 3 * 4 + 64 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/import",
            post(start_import).layer(DefaultBodyLimit::max(IMPORT_BODY_MAX_BYTES)),
        )
        .route("/api/v1/import/{import_job_id}", get(get_import_status))
}

/// Source `importHttpInput` (`z.strictObject`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartImportBody {
    workspace_id: Uuid,
    source: String,
    zip_base64: Option<String>,
    file_name: Option<String>,
    project_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportStatusQuery {
    workspace_id: Uuid,
}

fn invalid_input() -> AppError {
    AppError::from_code(ProblemCode::InvalidInput)
}

fn import_failed() -> AppError {
    AppError::from_code(ProblemCode::ImportFailed)
}

/// Source `parseHttpBody(…, "import")`: runs only after authentication, so an
/// anonymous caller never makes the server buffer or parse an upload.
async fn read_import_body(request: Request) -> Result<StartImportBody, AppError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    if content_type.as_deref() != Some("application/json") {
        return Err(AppError::from_code(ProblemCode::UnsupportedMediaType));
    }
    let bytes = axum::body::to_bytes(request.into_body(), IMPORT_BODY_MAX_BYTES)
        .await
        .map_err(|_| AppError::problem(StatusCode::PAYLOAD_TOO_LARGE, ProblemCode::InvalidInput))?;
    let body: StartImportBody = serde_json::from_slice(&bytes).map_err(|_| invalid_input())?;
    if let Some(name) = &body.file_name {
        let len = name.chars().count();
        if !(1..=255).contains(&len) {
            return Err(AppError::with_source(
                ProblemCode::InvalidInput,
                "/fileName",
            ));
        }
    }
    if body.zip_base64.as_deref() == Some("") {
        return Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/zipBase64",
        ));
    }
    Ok(body)
}

async fn start_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Request,
) -> Result<Response, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = crate::http::authz::require_request_auth(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let (user_id, session_id) = (auth.user_id, auth.credential_id);
    let body = read_import_body(request).await?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("import-user:{user_id}"),
            5,
            std::time::Duration::from_secs(15 * 60),
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let source = ImportSource::parse(&body.source)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/source"))?;
    if source == ImportSource::NativeArchive {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/source"));
    }
    let file_bytes = match &body.zip_base64 {
        Some(b64) => B64
            .decode(b64.as_bytes())
            .map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/zipBase64"))?,
        None => Vec::new(),
    };
    drop(body.zip_base64);
    if file_bytes.len() > IMPORT_HTTP_MAX_BYTES {
        return Err(AppError::problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            ProblemCode::InvalidInput,
        ));
    }
    let pool = &state.auth.db.pool;

    if source == ImportSource::MarkdownZip {
        let seed = match state.collab.as_ref() {
            Some(hub) => Some(SeedEngine::from_hub(hub)),
            None => SeedEngine::from_env(),
        };
        let Some(seed) = seed else {
            tracing::error!("import.failed reason=collab_engine_unset");
            return Err(import_failed());
        };
        let Some(markdown_helper) = state.markdown.as_ref() else {
            tracing::error!("import.failed reason=markdown_helper_unset");
            return Err(import_failed());
        };
        let job = create_sync_import_job(pool, body.workspace_id, user_id, session_id)
            .await
            .map_err(internal)?
            .map_err(map_import_error)?;
        let created = match run_markdown_zip_import(
            pool,
            &seed,
            markdown_helper,
            body.workspace_id,
            job.id,
            user_id,
            session_id,
            file_bytes,
        )
        .await
        {
            Ok(ids) => ids,
            Err(SyncImportError::Failed(detail)) => {
                tracing::error!(
                    import_job_id = %job.id,
                    error_hash = %hash(&detail),
                    "import.failed"
                );
                return Err(import_failed());
            }
            Err(SyncImportError::NotFound) => {
                return Err(AppError::from_code(ProblemCode::NotFound))
            }
            Err(SyncImportError::Db(err)) => return Err(internal(err)),
        };
        return Ok(created_response(
            job.id,
            body.workspace_id,
            source,
            "completed",
            &created,
        ));
    }

    // Source `startAsyncImport`: an instance without a runner fails before any
    // membership lookup; the format and file checks come before the row.
    let Some(wake) = state.import_wake.as_ref() else {
        tracing::error!(
            source = source.as_str(),
            "import.failed reason=source_unavailable"
        );
        return Err(import_failed());
    };
    if file_bytes.is_empty() {
        return Err(import_failed());
    }
    if source == ImportSource::OfficeFile
        && !office_format_supported(
            body.file_name.as_deref().unwrap_or(""),
            state.import_extractor_available,
        )
    {
        return Err(import_failed());
    }
    let job = create_async_import_job(
        pool,
        body.workspace_id,
        user_id,
        session_id,
        source,
        NewAsyncImport {
            file_name: body.file_name.as_deref(),
            project_id: body.project_id,
            payload: &file_bytes,
        },
    )
    .await
    .map_err(internal)?
    .map_err(map_import_error)?;
    wake.notify_one();
    Ok(created_response(
        job.id,
        body.workspace_id,
        source,
        "running",
        &[],
    ))
}

fn created_response(
    id: Uuid,
    workspace_id: Uuid,
    source: ImportSource,
    status: &str,
    created: &[Uuid],
) -> Response {
    (
        StatusCode::CREATED,
        Json(ImportJobResponse {
            id: id.to_string(),
            workspace_id: workspace_id.to_string(),
            source: source.as_str().to_string(),
            status: status.to_string(),
            created_document_ids: created.iter().map(Uuid::to_string).collect(),
        }),
    )
        .into_response()
}

async fn get_import_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(import_job_id): Path<Uuid>,
    Query(query): Query<ImportStatusQuery>,
) -> Result<Json<ImportJobResponse>, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = crate::http::authz::require_request_auth(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let job = get_import_job(
        &state.auth.db.pool,
        query.workspace_id,
        auth.user_id,
        auth.credential_id,
        import_job_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_import_error)?;
    Ok(Json(ImportJobResponse {
        id: job.id.to_string(),
        workspace_id: job.workspace_id.to_string(),
        source: job.source.as_str().to_string(),
        status: job.status.as_str().to_string(),
        created_document_ids: job
            .created_refs
            .document_ids
            .iter()
            .map(Uuid::to_string)
            .collect(),
    }))
}

fn map_import_error(err: ImportDbError) -> AppError {
    match err {
        ImportDbError::NotFound | ImportDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn hash(detail: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(detail.as_bytes()))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

#[cfg(test)]
mod selected_import_route_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::backend::Backend;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state(backend: Backend, storage: crate::attachments::ObjectStorage) -> AppState {
        AppState {
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(backend),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage,
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:Some(Arc::new(tokio::sync::Notify::new())),import_extractor_available:false,preview_extract:None,quota:Default::default(),mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }
    async fn session(f: &Fixture) -> (Uuid, String) {
        let token = crate::auth::token::new_token();
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (id, token.token)
    }
    async fn request(
        app: Router,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("origin", "http://localhost")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("cookie", format!("fvoci_session={token}"));
        }
        let req = builder
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    fn body(workspace: Uuid) -> Value {
        serde_json::json!({"workspaceId":workspace,"source":"office-file","fileName":"normal.md","zipBase64":B64.encode("literal import 한글 😀")})
    }

    #[tokio::test]
    async fn import_selected_http_submit_real_queue_claim_fresh_status_and_denials() {
        let f = Fixture::new().await;
        let (credential, token) = session(&f).await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("import-http-storage"));
        let app = router().with_state(state(f.backend.clone(), storage.clone()));
        let (anonymous, _) = request(
            app.clone(),
            "POST",
            "/api/v1/import",
            None,
            body(f.workspace),
        )
        .await;
        assert_eq!(anonymous, StatusCode::UNAUTHORIZED);
        let (status, created) = request(
            app.clone(),
            "POST",
            "/api/v1/import",
            Some(&token),
            body(f.workspace),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["status"], "running");
        let job = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let claim = crate::db::import_jobs::claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.job_id, job);
        assert_eq!(claim.session_id, credential);
        assert_eq!(
            crate::db::import_jobs::load_import_payload_backend(&f.backend, &claim)
                .await
                .unwrap()
                .unwrap(),
            "literal import 한글 😀".as_bytes()
        );
        let fresh_pool = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let (_, fresh_token) = session(&f).await;
        let fresh = router().with_state(state(Backend::Sqlite(fresh_pool.clone()), storage));
        let path = format!("/api/v1/import/{job}?workspaceId={}", f.workspace);
        let (status, observed) =
            request(fresh.clone(), "GET", &path, Some(&fresh_token), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(observed["id"], created["id"]);
        assert_eq!(observed["status"], "running");
        assert_eq!(observed["createdDocumentIds"], serde_json::json!([]));
        assert!(observed.get("payload").is_none());
        assert!(observed.get("leaseToken").is_none());
        let wrong = format!("/api/v1/import/{job}?workspaceId={}", Uuid::now_v7());
        assert_eq!(
            request(fresh, "GET", &wrong, Some(&fresh_token), Value::Null)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            request(app, "GET", &path, Some(&token), Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        fresh_pool.close().await;
        f.close().await;
    }

    /// This case requires the allocated real server/engine binaries. It fails
    /// on missing native inputs rather than skipping or substituting a mock.
    #[cfg(feature = "db-tests")]
    #[tokio::test]
    async fn import_selected_http_native_consumer_literal_ids_receipts_and_fresh_read() {
        let server = std::path::PathBuf::from(
            std::env::var("FVOCI_IMPORT_TEST_SERVER_BIN")
                .expect("allocated native fvoci-server path required"),
        );
        let seed = SeedEngine::from_env().expect("allocated FVOCI_COLLAB_ENGINE required");
        let helper = crate::documents::markdown_helper::MarkdownHelper::new(&server);
        let settings = crate::import_job::ImportJobSettings {
            office_helper: Some(server),
            markdown: Some(helper.clone()),
            seed: Some(seed),
            ..crate::import_job::ImportJobSettings::from_env()
        };
        let f = Fixture::new().await;
        let (_, token) = session(&f).await;
        let storage = crate::attachments::ObjectStorage::local(f.root.join("import-http-storage"));
        let mut initial = state(f.backend.clone(), storage.clone());
        initial.markdown = Some(helper.clone());
        let app = router().with_state(initial);
        let (status, created) = request(
            app,
            "POST",
            "/api/v1/import",
            Some(&token),
            body(f.workspace),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let job = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        assert!(crate::import_job::run_next_import_backend(
            &f.backend,
            &settings,
            &storage,
            &tokio_util::sync::CancellationToken::new()
        )
        .await
        .unwrap());
        assert!(!crate::import_job::run_next_import_backend(
            &f.backend,
            &settings,
            &storage,
            &tokio_util::sync::CancellationToken::new()
        )
        .await
        .unwrap());
        let fresh_pool = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let (fresh_credential, fresh_token) = session(&f).await;
        let fresh_backend = Backend::Sqlite(fresh_pool.clone());
        let mut fresh_state = state(fresh_backend.clone(), storage);
        fresh_state.markdown = Some(helper);
        let fresh = router().with_state(fresh_state);
        let path = format!("/api/v1/import/{job}?workspaceId={}", f.workspace);
        let (status, observed) =
            request(fresh, "GET", &path, Some(&fresh_token), Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(observed["status"], "completed");
        let ids = observed["createdDocumentIds"].as_array().unwrap();
        assert_eq!(ids.len(), 1);
        let document = Uuid::parse_str(ids[0].as_str().unwrap()).unwrap();
        assert_ne!(document, f.document);
        let read = crate::db::documents::read_wiki_document_body_backend(
            &fresh_backend,
            f.workspace,
            f.user,
            fresh_credential,
            document,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(read.id, document);
        assert_eq!(read.title, "normal");
        assert_eq!(
            read.schema_version,
            crate::db::documents::DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(read.status, "draft");
        assert_eq!(
            read.content_json,
            serde_json::json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"literal import 한글 😀"}]}]})
        );
        let (updates,receipts):(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2),(SELECT count(*) FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2)").bind(f.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_one(&fresh_pool).await.unwrap();
        assert_eq!((updates, receipts), (1, 1));
        let current: (String, i64, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT status,attempts,payload,lease_token FROM import_jobs WHERE id=?1",
        )
        .bind(job.as_bytes().as_slice())
        .fetch_one(&fresh_pool)
        .await
        .unwrap();
        assert_eq!(current, ("completed".into(), 1, None, None));
        fresh_pool.close().await;
        f.close().await;
    }
}
