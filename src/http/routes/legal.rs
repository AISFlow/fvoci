//! Public legal documents, consents, public instance settings and branding
//! asset delivery (source `domains/admin/{legal,branding,instance-settings}.ts`,
//! `domains/identity/consent.ts`).

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::dto::{
    ConsentsPendingResponse, ConsentsSubmitBody, InstanceSettingsOutput, LegalVersionMetaOutput,
    LegalVersionQuery, LegalVersionsResponse, MemberConsentOutput, OkResponse,
    PublicBrandingOutput, PublicSettingsValues, WorkspaceConsentsResponse,
    WorkspaceMemberConsentsOutput,
};
use crate::db::legal::{
    is_legal_kind, latest_legal, legal_version, list_legal_versions, pending_consents,
    record_consents, workspace_consents, RecordConsentsOutcome,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::admin::{
    if_none_matches, legal_output, not_modified, strong_etag, BRANDING_ASSET_MAX_BYTES,
};
use crate::http::state::AppState;
use crate::settings::{self, asset_href, BrandingAssetKind};

/// Same 60 s as the asset bytes: the two public surfaces must not drift.
const PUBLIC_CACHE_CONTROL: &str = "public, max-age=60";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/legal/{kind}", get(get_legal))
        .route("/api/v1/legal/{kind}/versions", get(get_legal_versions))
        .route("/api/v1/auth/consents", post(post_consents))
        .route("/api/v1/auth/consents/pending", get(get_pending_consents))
        .route(
            "/api/v1/workspaces/{workspace_id}/consents",
            get(get_workspace_consents),
        )
        .route("/api/v1/instance", get(get_instance))
        .route("/api/v1/branding/{asset}", get(get_branding_asset))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

fn not_found() -> AppError {
    AppError::from_code(ProblemCode::NotFound)
}

fn kind_param(kind: &str) -> Result<(), AppError> {
    if is_legal_kind(kind) {
        Ok(())
    } else {
        Err(AppError::with_source(ProblemCode::InvalidInput, "/kind"))
    }
}

/// `z.coerce.number().int().positive()`.
fn version_param(raw: &str) -> Result<i32, AppError> {
    let trimmed = raw.trim();
    let invalid = || AppError::from_code(ProblemCode::InvalidInput);
    let value: f64 = if trimmed.is_empty() {
        0.0
    } else {
        trimmed.parse().map_err(|_| invalid())?
    };
    if value.fract() != 0.0 || value < 1.0 || value > i32::MAX as f64 {
        return Err(invalid());
    }
    Ok(value as i32)
}

async fn get_legal(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    query: Result<Query<LegalVersionQuery>, QueryRejection>,
) -> Result<Json<crate::api::dto::LegalDocumentOutput>, AppError> {
    kind_param(&kind)?;
    let Query(query) = query.map_err(AppError::from)?;
    let pool = &state.auth.db.pool;
    let doc = match query.version.as_deref() {
        None => latest_legal(pool, &kind).await,
        Some(raw) => legal_version(pool, &kind, version_param(raw)?).await,
    }
    .map_err(internal)?
    .ok_or_else(not_found)?;
    Ok(Json(legal_output(doc)))
}

async fn get_legal_versions(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<Json<LegalVersionsResponse>, AppError> {
    kind_param(&kind)?;
    let docs = list_legal_versions(&state.auth.db.pool, &kind)
        .await
        .map_err(internal)?;
    Ok(Json(LegalVersionsResponse {
        versions: docs
            .into_iter()
            .map(|doc| LegalVersionMetaOutput {
                kind: doc.kind,
                version: doc.version,
                title: doc.title,
                effective_at: doc.effective_at,
                required: doc.required,
                published_at: doc.published_at,
            })
            .collect(),
    }))
}

async fn get_pending_consents(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ConsentsPendingResponse>, AppError> {
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let pending = pending_consents(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?;
    Ok(Json(ConsentsPendingResponse {
        pending: pending.into_iter().map(legal_output).collect(),
    }))
}

async fn post_consents(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<ConsentsSubmitBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let Json(body) = body.map_err(AppError::from)?;
    if body.items.is_empty() {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/items"));
    }
    // Versions outside i32 cannot name a document; they are skipped like
    // any stale version.
    let items: Vec<(String, i32)> = body
        .items
        .into_iter()
        .filter_map(|item| i32::try_from(item.version).ok().map(|v| (item.kind, v)))
        .collect();
    let ip = peer_ip(peer.ip());
    match record_consents(&state.auth.db.pool, auth.user_id, &items, Some(&ip))
        .await
        .map_err(internal)?
    {
        RecordConsentsOutcome::Recorded => Ok(Json(OkResponse { ok: true })),
        RecordConsentsOutcome::NotLive => {
            Err(AppError::from_code(ProblemCode::AuthenticationRequired))
        }
    }
}

async fn get_workspace_consents(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WorkspaceConsentsResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    let members = workspace_consents(&state.auth.db.pool, workspace_id, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(WorkspaceConsentsResponse {
        members: members
            .into_iter()
            .map(|member| WorkspaceMemberConsentsOutput {
                user_id: member.user_id.to_string(),
                consents: member
                    .consents
                    .into_iter()
                    .map(|(kind, version, consented_at)| MemberConsentOutput {
                        kind,
                        version,
                        consented_at,
                    })
                    .collect(),
            })
            .collect(),
    }))
}

/// Anonymous instance settings: only `public` catalog keys, branding assets
/// as delivery paths. Cached 60 s with a strong ETag over the body.
async fn get_instance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let snapshot = settings::load(
        &state.auth.db.pool,
        &state.auth.db.settings_boot,
        &state.branding_name,
    )
    .await
    .map_err(internal)?;
    let values = snapshot.values;
    let body = InstanceSettingsOutput {
        version: snapshot.revision,
        values: PublicSettingsValues {
            branding: PublicBrandingOutput {
                name: values.branding.name.clone(),
                logo: asset_href(BrandingAssetKind::Logo, values.branding.logo.as_ref()),
                favicon: asset_href(BrandingAssetKind::Favicon, values.branding.favicon.as_ref()),
                login_brand_text: values.branding.login_brand_text.clone(),
            },
            defaults_user: values.defaults_user,
            share: values.share,
            features: values.features,
            attachment_preview: values.attachment_preview,
            operator: values.operator,
            web_push_public_key: None,
        },
    };
    let bytes = serde_json::to_vec(&body).map_err(|_| AppError::internal())?;
    let tag = strong_etag(&bytes);
    if if_none_matches(&headers, &tag) {
        return Ok(not_modified(&tag, PUBLIC_CACHE_CONTROL));
    }
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PUBLIC_CACHE_CONTROL),
    );
    if let Ok(value) = HeaderValue::from_str(&tag) {
        headers.insert(header::ETAG, value);
    }
    Ok(response)
}

/// Serves an uploaded logo/favicon. The stored digest must match the bytes:
/// the setting names exactly what the upload route wrote, so this anonymous
/// route cannot be pointed at another object. `nosniff` + `sandbox` CSP keep
/// the response from ever executing as a document.
async fn get_branding_asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(asset): Path<String>,
) -> Result<Response, AppError> {
    let kind = BrandingAssetKind::parse(&asset)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/asset"))?;
    let values = settings::current_values(&state.auth.db.pool, &state.branding_name)
        .await
        .map_err(internal)?;
    let asset = values.branding.asset(kind).cloned().ok_or_else(not_found)?;
    let tag = strong_etag(asset.sha256.as_bytes());
    if if_none_matches(&headers, &tag) {
        return Ok(not_modified(&tag, PUBLIC_CACHE_CONTROL));
    }
    let key = asset.key.to_string();
    let size = state.storage.head(&key).await.map_err(|err| {
        tracing::warn!(error = %err, "branding asset head failed");
        not_found()
    })?;
    let Some(size) = size.filter(|s| *s > 0 && *s <= BRANDING_ASSET_MAX_BYTES as u64) else {
        return Err(not_found());
    };
    let bytes = state
        .storage
        .read_range(&key, 0, size - 1)
        .await
        .map_err(|err| {
            tracing::warn!(error = %err, "branding asset read failed");
            not_found()
        })?;
    if hex::encode(Sha256::digest(&bytes)) != asset.sha256 {
        tracing::warn!(key = %format!("branding.{}", kind.as_str()), reason = "asset_digest", "settings.row_invalid");
        return Err(not_found());
    }
    let mut response = Response::new(Body::from(bytes));
    let out = response.headers_mut();
    out.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    out.insert(
        "content-security-policy",
        HeaderValue::from_static("sandbox"),
    );
    out.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PUBLIC_CACHE_CONTROL),
    );
    if let Ok(value) = HeaderValue::from_str(&asset.mime) {
        out.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&tag) {
        out.insert(header::ETAG, value);
    }
    Ok(response)
}
