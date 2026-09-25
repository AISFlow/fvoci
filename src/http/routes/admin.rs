//! Instance administration routes (source `apps/server/src/domains/admin`).
//!
//! Every route is session-only (API tokens get 404) and answers 404 to a
//! caller who is not a live instance admin, like the source's masked
//! `ForbiddenError`. The admin check itself runs inside each operation's
//! transaction (`crate::db::admin`).

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use chrono::Duration;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::dto::{
    AdminInstanceSettingsOutput, AdminSystemOutput, AdminUserItemOutput, AdminUserListResponse,
    AdminUserPatchBody, AdminUserPatchOutput, AdminWorkspaceItemOutput, AdminWorkspaceListResponse,
    AuditLogItemOutput, AuditLogListResponse, AuditLogQuery, InstanceAdminBody,
    LegalDocumentOutput, LegalPublishBody, OkResponse,
};
use crate::db::admin::{
    decode_audit_cursor, instance_directory, list_audit, list_users, list_workspaces,
    patch_instance_user, InstanceUserPatch, PatchUserOutcome,
};
use crate::db::legal::{is_legal_kind, publish_legal, LegalDocument, LegalPublishInput};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::settings::catalog::parse_patch_value;
use crate::settings::{
    self, BrandingAsset, BrandingAssetKind, SettingsChange, SettingsKey, SettingsSnapshot,
    SettingsWriteError, BRANDING_ASSET_MIME,
};
use crate::validate::{parse_iso_datetime, utf16_len};

/// Source `ASSET_MAX_BYTES`.
pub const BRANDING_ASSET_MAX_BYTES: usize = 512 * 1024;
/// Withdrawal grace period (source `getWithdrawalDeadline`).
const ERASE_AFTER_DAYS: i64 = 14;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/audit", get(get_audit))
        .route("/api/v1/admin/system", get(get_system))
        .route("/api/v1/admin/users", get(get_users).patch(patch_users))
        .route("/api/v1/admin/workspaces", get(get_workspaces))
        .route(
            "/api/v1/admin/instance-settings",
            get(get_instance_settings).patch(patch_instance_settings),
        )
        .route(
            "/api/v1/admin/instance-admins",
            patch(patch_instance_admins),
        )
        .route("/api/v1/admin/legal", post(post_legal))
        .route(
            "/api/v1/admin/branding/assets/{asset}",
            post(upload_branding_asset).delete(remove_branding_asset),
        )
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", sanitize_db_error(&err));
    AppError::internal()
}

fn sanitize_db_error(err: &sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    }
}

fn not_found() -> AppError {
    AppError::from_code(ProblemCode::NotFound)
}

async fn session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<RequestAuth, AppError> {
    require_request_auth(state, headers, jar, Access::Session, None).await
}

// ---------------------------------------------------------------- audit

fn audit_limit(raw: Option<&str>) -> Result<i64, AppError> {
    let Some(raw) = raw else { return Ok(50) };
    // z.coerce.number(): Number("") is 0 and whitespace trims.
    let trimmed = raw.trim();
    let value: f64 = if trimmed.is_empty() {
        0.0
    } else {
        trimmed
            .parse()
            .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    };
    if value.fract() != 0.0 || !(1.0..=100.0).contains(&value) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    Ok(value as i64)
}

fn invalid_cursor() -> AppError {
    AppError {
        status: StatusCode::BAD_REQUEST,
        code: ProblemCode::InvalidInput,
        source: None,
        params: Some(serde_json::json!({ "code": "invalid_cursor" })),
        retry_after: None,
    }
}

async fn get_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    query: Result<Query<AuditLogQuery>, QueryRejection>,
) -> Result<Json<AuditLogListResponse>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let limit = audit_limit(query.limit.as_deref())?;
    let cursor = match query.cursor.as_deref() {
        None => None,
        Some(raw) if raw.len() > 1024 => {
            return Err(AppError::from_code(ProblemCode::InvalidInput))
        }
        Some(raw) => Some(decode_audit_cursor(raw).ok_or_else(invalid_cursor)?),
    };
    let page = list_audit(&state.auth.db.pool, auth.user_id, cursor, limit)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(AuditLogListResponse {
        items: page
            .items
            .into_iter()
            .map(|row| AuditLogItemOutput {
                id: row.id.to_string(),
                actor_user_id: row.actor_user_id.map(|id| id.to_string()),
                workspace_id: row.workspace_id.map(|id| id.to_string()),
                verb: row.verb,
                target_type: row.target_type,
                target_id: row.target_id.map(|id| id.to_string()),
                payload: match row.payload {
                    Value::Object(map) => Value::Object(map),
                    _ => Value::Object(Default::default()),
                },
                ip: row.ip,
                created_at: row.created_at,
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

// ---------------------------------------------------------------- directory

async fn get_system(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminSystemOutput>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let dir = instance_directory(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(AdminSystemOutput {
        users: dir.users,
        workspaces: dir.workspaces,
        documents: dir.documents,
        tasks: dir.tasks,
    }))
}

async fn get_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminUserListResponse>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let rows = list_users(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(AdminUserListResponse {
        items: rows
            .into_iter()
            .map(|row| AdminUserItemOutput {
                id: row.id.to_string(),
                email: row.email,
                given_name: row.given_name,
                family_name: row.family_name,
                instance_admin: row.instance_admin,
                suspended_at: row.suspended_at,
                deleted_at: row.deleted_at,
                erase_at: row
                    .deleted_at
                    .map(|at| at + Duration::days(ERASE_AFTER_DAYS)),
                created_at: row.created_at,
            })
            .collect(),
    }))
}

async fn get_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminWorkspaceListResponse>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    let rows = list_workspaces(&state.auth.db.pool, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(AdminWorkspaceListResponse {
        items: rows
            .into_iter()
            .map(|row| AdminWorkspaceItemOutput {
                id: row.id.to_string(),
                slug: row.slug,
                name: row.name,
                created_at: row.created_at,
            })
            .collect(),
    }))
}

fn map_patch_outcome(outcome: PatchUserOutcome) -> Result<AdminUserPatchOutput, AppError> {
    match outcome {
        PatchUserOutcome::Ok { suspended_at } => Ok(AdminUserPatchOutput {
            ok: true,
            suspended_at,
        }),
        PatchUserOutcome::NotFound => Err(not_found()),
        PatchUserOutcome::LastInstanceAdmin => {
            Err(AppError::from_code(ProblemCode::LastInstanceAdmin))
        }
        PatchUserOutcome::SelfSuspension => Err(AppError::from_code(ProblemCode::SelfSuspension)),
        PatchUserOutcome::SeatLimit => Err(AppError::from_code(ProblemCode::LimitSeats)),
    }
}

async fn patch_users(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<AdminUserPatchBody>, JsonRejection>,
) -> Result<Json<AdminUserPatchOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let Json(body) = body.map_err(AppError::from)?;
    if body.instance_admin.is_none() && body.suspended.is_none() {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
    }
    let ip = peer_ip(peer.ip());
    let outcome = patch_instance_user(
        &state.auth.db.pool,
        auth.user_id,
        body.user_id,
        InstanceUserPatch {
            instance_admin: body.instance_admin,
            suspended: body.suspended,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .ok_or_else(not_found)?;
    // Suspension revoked every session and token in the same transaction;
    // open collab sockets close on their next credential poll.
    Ok(Json(map_patch_outcome(outcome)?))
}

/// Source `setInstanceAdmin`: the same operation with only the admin flag;
/// self-suspension cannot occur, and success is `{ ok: true }`.
async fn patch_instance_admins(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<InstanceAdminBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let ip = peer_ip(peer.ip());
    let outcome = patch_instance_user(
        &state.auth.db.pool,
        auth.user_id,
        body.user_id,
        InstanceUserPatch {
            instance_admin: Some(body.value),
            suspended: None,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .ok_or_else(not_found)?;
    map_patch_outcome(outcome)?;
    Ok(Json(OkResponse { ok: true }))
}

// ---------------------------------------------------------------- legal

pub fn legal_output(doc: LegalDocument) -> LegalDocumentOutput {
    LegalDocumentOutput {
        kind: doc.kind,
        version: doc.version,
        title: doc.title,
        body_html: doc.body_html,
        effective_at: doc.effective_at,
        required: doc.required,
        published_at: doc.published_at,
    }
}

async fn post_legal(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<LegalPublishBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let title = body.title.trim().to_string();
    let effective_at = parse_iso_datetime(&body.effective_at);
    let valid = is_legal_kind(&body.kind)
        && (1..=300).contains(&utf16_len(&title))
        && (1..=200_000).contains(&utf16_len(&body.body_markdown))
        && effective_at.is_some();
    let Some(effective_at) = effective_at.filter(|_| valid) else {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
    };
    let Some(convert) = state.document_convert.as_ref() else {
        tracing::error!("legal publish requested but FVOCI_DOCUMENT_CONVERT_BIN is unset");
        return Err(AppError::internal());
    };
    let body_html = convert
        .md_to_safe_html(&body.body_markdown)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "legal body render failed");
            AppError::internal()
        })?;
    let ip = peer_ip(peer.ip());
    let doc = publish_legal(
        &state.auth.db.pool,
        auth.user_id,
        LegalPublishInput {
            kind: body.kind,
            title,
            body_markdown: body.body_markdown,
            body_html,
            required: body.required,
            effective_at,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .ok_or_else(not_found)?;
    Ok((StatusCode::CREATED, Json(legal_output(doc))).into_response())
}

// ---------------------------------------------------------------- settings

pub fn admin_settings_output(
    state: &AppState,
    snapshot: &SettingsSnapshot,
) -> AdminInstanceSettingsOutput {
    let boot = settings::boot_values(&state.auth.db.settings_boot, &snapshot.values);
    AdminInstanceSettingsOutput {
        version: snapshot.revision,
        values: snapshot.values.clone(),
        overridden: snapshot
            .overridden
            .iter()
            .map(|k| k.as_str().to_string())
            .collect(),
        restart_required: snapshot
            .restart_pending(boot)
            .iter()
            .map(|k| k.as_str().to_string())
            .collect(),
        env_applied: snapshot.env_applied.clone(),
        ee_features: settings::EE_FEATURES_ENABLED
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

async fn require_admin_read(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    let mut tx = state.auth.db.pool.begin().await.map_err(internal)?;
    let ok = crate::db::admin::require_live_instance_admin(&mut tx, user_id)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    if ok {
        Ok(())
    } else {
        Err(not_found())
    }
}

async fn get_instance_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<AdminInstanceSettingsOutput>, AppError> {
    let auth = session(&state, &headers, &jar).await?;
    require_admin_read(&state, auth.user_id).await?;
    let snapshot = settings::load(
        &state.auth.db.pool,
        &state.auth.db.settings_boot,
        &state.branding_name,
    )
    .await
    .map_err(internal)?;
    Ok(Json(admin_settings_output(&state, &snapshot)))
}

/// Source `instanceSettingsPatchInput`: a strict object of catalog keys, at
/// least one, each `null` (reset) or a value of that key's schema.
fn parse_settings_patch(body: &Value) -> Result<Vec<(SettingsKey, Option<Value>)>, AppError> {
    let invalid = |key: &str| AppError::with_source(ProblemCode::InvalidInput, format!("/{key}"));
    let object = body
        .as_object()
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/"))?;
    if object.is_empty() {
        return Err(AppError::with_source(ProblemCode::InvalidInput, "/"));
    }
    let mut items = Vec::with_capacity(object.len());
    for (key, value) in object {
        let parsed = SettingsKey::parse(key).ok_or_else(|| invalid(key))?;
        if value.is_null() {
            items.push((parsed, None));
            continue;
        }
        let normalized = parse_patch_value(parsed, value).ok_or_else(|| invalid(key))?;
        items.push((parsed, Some(normalized)));
    }
    Ok(items)
}

fn map_settings_write(err: SettingsWriteError) -> AppError {
    match err {
        SettingsWriteError::NotAdmin | SettingsWriteError::AssetMissing => not_found(),
    }
}

async fn patch_instance_settings(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<AdminInstanceSettingsOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let items = parse_settings_patch(&body)?;
    let ip = peer_ip(peer.ip());
    let outcome = settings::apply_change(
        &state.auth.db.pool,
        auth.user_id,
        Some(&ip),
        &state.branding_name,
        SettingsChange::Patch(items),
    )
    .await
    .map_err(internal)?
    .map_err(map_settings_write)?;
    Ok(Json(admin_settings_output(&state, &outcome.snapshot)))
}

// ---------------------------------------------------------------- branding assets

fn parse_asset_kind(raw: &str) -> Result<BrandingAssetKind, AppError> {
    BrandingAssetKind::parse(raw)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/asset"))
}

/// Source `sniffMimeFromBytes` + `file-type`: `infer` reports APNG as PNG, so
/// an `acTL` chunk before the first `IDAT` marks it as `image/apng`.
pub fn sniff_branding_mime(bytes: &[u8]) -> Option<&'static str> {
    let sniffed = crate::attachments::sniff_mime_from_bytes(bytes);
    let mime = if sniffed == "image/png" && is_apng(bytes) {
        "image/apng"
    } else {
        sniffed.as_str()
    };
    BRANDING_ASSET_MIME.into_iter().find(|m| *m == mime)
}

fn is_apng(bytes: &[u8]) -> bool {
    let mut pos = 8;
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = &bytes[pos + 4..pos + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" {
            return false;
        }
        pos = match pos.checked_add(12).and_then(|p| p.checked_add(len)) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

async fn store_branding_object(state: &AppState, key: Uuid, bytes: &[u8]) -> Result<(), AppError> {
    let storage_error = |err: crate::attachments::StorageError| {
        tracing::error!(error = %err, "branding asset store failed");
        AppError::internal()
    };
    let key = key.to_string();
    let storage = &state.storage;
    let upload_ref = storage
        .create_multipart(&key)
        .await
        .map_err(storage_error)?;
    let chunk: Result<bytes::Bytes, std::io::Error> = Ok(bytes::Bytes::copy_from_slice(bytes));
    let stream = futures_util::stream::iter(vec![chunk]);
    let result = async {
        let mut staged = storage
            .stage_part_stream(
                &key,
                upload_ref.as_deref(),
                1,
                stream,
                Some(bytes.len() as u64),
                BRANDING_ASSET_MAX_BYTES as u64,
            )
            .await?;
        let part = storage.publish_staged_part(&key, 1, &mut staged).await?;
        storage
            .assemble_multipart(&key, upload_ref.as_deref(), &[(1, part.etag)])
            .await?;
        storage.finalize_multipart(&key).await
    }
    .await;
    if let Err(err) = result {
        let _ = storage.purge_key(&key).await;
        return Err(storage_error(err));
    }
    Ok(())
}

async fn delete_branding_object(state: &AppState, asset: &BrandingAsset) {
    // After commit: a failure leaves an unreferenced object, never a
    // reference to a missing one.
    if let Err(err) = state.storage.purge_key(&asset.key.to_string()).await {
        tracing::warn!(error = %err, "branding asset delete failed");
    }
}

async fn upload_branding_asset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(asset): Path<String>,
    body: Body,
) -> Result<Json<AdminInstanceSettingsOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let kind = parse_asset_kind(&asset)?;
    require_admin_read(&state, auth.user_id).await?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    if content_type.as_deref() != Some("application/octet-stream") {
        return Err(AppError::from_code(ProblemCode::UnsupportedMediaType));
    }
    let too_large = || AppError::problem(StatusCode::PAYLOAD_TOO_LARGE, ProblemCode::InvalidInput);
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > BRANDING_ASSET_MAX_BYTES as u64) {
        return Err(too_large());
    }
    let bytes = axum::body::to_bytes(body, BRANDING_ASSET_MAX_BYTES)
        .await
        .map_err(|_| too_large())?;
    if bytes.is_empty() {
        return Err(AppError::from_code(
            ProblemCode::RawApplicationOctetStreamBodyRequired,
        ));
    }
    let mime = sniff_branding_mime(&bytes)
        .ok_or_else(|| AppError::from_code(ProblemCode::UnsupportedBrandingAssetType))?;
    let key = Uuid::now_v7();
    store_branding_object(&state, key, &bytes).await?;
    let record = BrandingAsset {
        key,
        sha256: hex::encode(Sha256::digest(&bytes)),
        mime: mime.to_string(),
    };
    let ip = peer_ip(peer.ip());
    let outcome = settings::apply_change(
        &state.auth.db.pool,
        auth.user_id,
        Some(&ip),
        &state.branding_name,
        SettingsChange::BrandingAsset {
            kind,
            asset: Some(record.clone()),
        },
    )
    .await;
    let outcome = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(err)) => {
            delete_branding_object(&state, &record).await;
            return Err(map_settings_write(err));
        }
        Err(err) => {
            delete_branding_object(&state, &record).await;
            return Err(internal(err));
        }
    };
    if let Some(previous) = outcome.previous_asset.as_ref() {
        delete_branding_object(&state, previous).await;
    }
    Ok(Json(admin_settings_output(&state, &outcome.snapshot)))
}

async fn remove_branding_asset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(asset): Path<String>,
) -> Result<Json<AdminInstanceSettingsOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = session(&state, &headers, &jar).await?;
    let kind = parse_asset_kind(&asset)?;
    let ip = peer_ip(peer.ip());
    let outcome = settings::apply_change(
        &state.auth.db.pool,
        auth.user_id,
        Some(&ip),
        &state.branding_name,
        SettingsChange::BrandingAsset { kind, asset: None },
    )
    .await
    .map_err(internal)?
    .map_err(map_settings_write)?;
    if let Some(previous) = outcome.previous_asset.as_ref() {
        delete_branding_object(&state, previous).await;
    }
    Ok(Json(admin_settings_output(&state, &outcome.snapshot)))
}

// ---------------------------------------------------------------- shared HTTP helpers

/// Source `strongEtag`: first 16 hex chars of SHA-256, quoted.
pub fn strong_etag(payload: &[u8]) -> String {
    format!("\"{}\"", &hex::encode(Sha256::digest(payload))[..16])
}

/// RFC 9110 weak comparison for `If-None-Match` (a CDN may weaken the tag).
pub fn if_none_matches(headers: &HeaderMap, tag: &str) -> bool {
    let Some(header) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if header.trim() == "*" {
        return true;
    }
    let weak = |v: &str| v.strip_prefix("W/").unwrap_or(v).to_string();
    let wanted = weak(tag);
    header.split(',').any(|token| weak(token.trim()) == wanted)
}

pub fn not_modified(tag: &str, cache_control: &'static str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(value) = HeaderValue::from_str(tag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_accepts_raster_only_and_detects_apng() {
        let png = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0,
            0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(sniff_branding_mime(&png), Some("image/png"));
        let mut apng = png[..33].to_vec();
        apng.extend_from_slice(&[
            0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        assert_eq!(sniff_branding_mime(&apng), Some("image/apng"));
        assert_eq!(
            sniff_branding_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(sniff_branding_mime(b"GIF89a\x01\x00\x01\x00"), None);
    }

    #[test]
    fn audit_limit_coerces_like_zod() {
        assert_eq!(audit_limit(None).unwrap(), 50);
        assert_eq!(audit_limit(Some(" 7 ")).unwrap(), 7);
        for bad in ["0", "101", "1.5", "x", ""] {
            assert!(audit_limit(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn etag_weak_comparison() {
        let tag = strong_etag(b"x");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_str(&format!("W/{tag}")).unwrap(),
        );
        assert!(if_none_matches(&headers, &tag));
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"other\""));
        assert!(!if_none_matches(&headers, &tag));
    }
}
