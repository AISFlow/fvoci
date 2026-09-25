// Path stubs for MFA / OIDC; merged into the main document by `spec_json`.

use utoipa::OpenApi;

use crate::api::dto::{
    MfaDisableBody, MfaEnableBody, MfaSetupBody, MfaSetupOutput, MfaStatusOutput, MfaVerifyBody,
    OkResponse, ProblemResponse, SessionIssuedOutput, WorkspaceOidcBody, WorkspaceOidcGetOutput,
    WorkspaceOidcOutput,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        mfa_status,
        mfa_setup,
        mfa_enable,
        mfa_disable,
        mfa_verify,
        auth_sso,
        oidc_start,
        oidc_callback,
        oidc_link,
        oidc_unlink,
        workspace_oidc_get,
        workspace_oidc_put,
        workspace_oidc_delete,
    ),
    components(schemas(
        MfaDisableBody,
        MfaEnableBody,
        MfaSetupBody,
        MfaSetupOutput,
        MfaStatusOutput,
        MfaVerifyBody,
        SessionIssuedOutput,
        WorkspaceOidcBody,
        WorkspaceOidcGetOutput,
        WorkspaceOidcOutput,
    ))
)]
pub struct IdentityApiDoc;

#[utoipa::path(
    get,
    path = "/api/v1/auth/mfa",
    tag = "auth",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "MFA state", body = MfaStatusOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn mfa_status() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/mfa/setup",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = MfaSetupBody,
    responses(
        (status = 200, description = "New secret and recovery codes (not yet enabled)", body = MfaSetupOutput),
        (status = 400, description = "mfa_password_invalid", body = ProblemResponse),
        (status = 401, description = "Authentication required or mfa_reauth_required", body = ProblemResponse),
        (status = 409, description = "mfa_already_enabled", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "encryption_unavailable", body = ProblemResponse),
    )
)]
fn mfa_setup() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/mfa/enable",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = MfaEnableBody,
    responses(
        (status = 200, description = "Enabled", body = OkResponse),
        (status = 400, description = "mfa_code_invalid or mfa_not_setup", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn mfa_enable() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/mfa/disable",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = MfaDisableBody,
    responses(
        (status = 200, description = "Disabled", body = OkResponse),
        (status = 400, description = "mfa_confirm_invalid or mfa_not_enabled", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn mfa_disable() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/mfa/verify",
    tag = "auth",
    request_body = MfaVerifyBody,
    responses(
        (status = 200, description = "Second factor accepted; session cookie set", body = SessionIssuedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "mfa_invalid", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn mfa_verify() {}

#[utoipa::path(
    get,
    path = "/api/v1/auth/sso",
    tag = "auth",
    params(("slug" = String, Query, description = "Workspace slug")),
    responses(
        (status = 302, description = "Redirect to the workspace identity provider; sets fvoci_oidc_state"),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "provider_not_configured", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn auth_sso() {}

#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/{provider}/start",
    tag = "auth",
    params(
        ("provider" = String, Path, description = "google | microsoft | kakao | naver | generic"),
        ("invitation" = Option<String>, Query, description = "Invitation token: accept with this identity"),
        ("consents" = Option<String>, Query, description = "JSON array of {kind, version}"),
        ("workspaceId" = Option<String>, Query, description = "Workspace SSO (generic)"),
    ),
    responses(
        (status = 302, description = "Redirect to the provider; sets fvoci_oidc_state"),
        (status = 400, description = "Invalid input or invalid_consents_query", body = ProblemResponse),
        (status = 404, description = "provider_not_configured", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn oidc_start() {}

#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/{provider}/callback",
    tag = "auth",
    params(("provider" = String, Path, description = "Provider key")),
    responses(
        (status = 302, description = "To `/` with a session, `/login#mfa=<token>`, `/settings/account?linked=1`, or `/login?error=<oidc code>` / `/settings/account?error=<oidc code>`"),
        (status = 402, description = "Seat limit", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn oidc_callback() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/oidc/{provider}/link",
    tag = "auth",
    security(("fvoci_session" = [])),
    params(
        ("provider" = String, Path, description = "Provider key"),
        ("workspaceId" = Option<String>, Query, description = "Workspace SSO (generic)"),
    ),
    responses(
        (status = 303, description = "Redirect to the provider; sets fvoci_oidc_state"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "provider_not_configured", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn oidc_link() {}

#[utoipa::path(
    post,
    path = "/api/v1/auth/oidc/{provider}/unlink",
    tag = "auth",
    security(("fvoci_session" = [])),
    params(("provider" = String, Path, description = "Provider key")),
    responses(
        (status = 200, description = "Unlinked", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "identity_link_not_found", body = ProblemResponse),
        (status = 409, description = "oidc_last_method", body = ProblemResponse),
    )
)]
fn oidc_unlink() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/oidc",
    tag = "workspaces",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace SSO configuration (secret never returned)", body = WorkspaceOidcGetOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
    )
)]
fn workspace_oidc_get() {}

#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/oidc",
    tag = "workspaces",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, Path, description = "Workspace id")),
    request_body = WorkspaceOidcBody,
    responses(
        (status = 200, description = "Saved", body = WorkspaceOidcOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
        (status = 503, description = "encryption_unavailable", body = ProblemResponse),
    )
)]
fn workspace_oidc_put() {}

#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/oidc",
    tag = "workspaces",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "Removed", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
    )
)]
fn workspace_oidc_delete() {}
