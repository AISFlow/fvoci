// Path stubs exist only for OpenAPI generation.

#[cfg(feature = "api-schema")]
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
#[cfg(feature = "api-schema")]
use utoipa::{Modify, OpenApi};

#[cfg(feature = "api-schema")]
use crate::api::dto::{
    BrandingOutput, CreateWorkspaceBody, LoginBody, LoginResponse, MemberResponse, MemberRoleBody,
    OkResponse, PatchMeBody, PatchWorkspaceBody, ProblemResponse, SessionUserOutput, SetupBody,
    SetupResponse, SetupStatusResponse, WorkspaceListItemResponse, WorkspaceListResponse,
    WorkspaceMetaResponse,
};

#[cfg(feature = "api-schema")]
struct CookieSecurityAddon;

#[cfg(feature = "api-schema")]
impl Modify for CookieSecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "fvoci_session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("fvoci_session"))),
        );
    }
}

#[cfg(feature = "api-schema")]
#[derive(OpenApi)]
#[openapi(
    info(
        title = "FVOCI API",
        version = "0.1.0",
        description = "Rust slice HTTP contract for authentication and workspace operations."
    ),
    paths(
        setup_status,
        setup_run,
        login,
        logout,
        me_get,
        me_patch,
        list_my_workspaces,
        personal_workspace,
        create_workspace,
        get_workspace,
        patch_workspace,
        patch_member,
        remove_member,
    ),
    components(
        schemas(
            SetupStatusResponse,
            BrandingOutput,
            SetupBody,
            SetupResponse,
            LoginBody,
            LoginResponse,
            SessionUserOutput,
            PatchMeBody,
            WorkspaceListResponse,
            WorkspaceListItemResponse,
            WorkspaceMetaResponse,
            OkResponse,
            MemberResponse,
            CreateWorkspaceBody,
            PatchWorkspaceBody,
            MemberRoleBody,
            ProblemResponse,
        )
    ),
    modifiers(&CookieSecurityAddon),
    tags(
        (name = "setup", description = "Instance setup"),
        (name = "auth", description = "Authentication and profile"),
        (name = "workspaces", description = "Workspace membership and metadata"),
    )
)]
pub struct ApiDoc;

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/setup",
    tag = "setup",
    responses(
        (status = 200, description = "Setup status", body = SetupStatusResponse),
        (status = 500, description = "Internal error", body = ProblemResponse),
    )
)]
fn setup_status() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/setup",
    tag = "setup",
    request_body = SetupBody,
    responses(
        (status = 201, description = "Created", body = SetupResponse),
        (status = 404, description = "Already completed", body = ProblemResponse),
        (status = 409, description = "Slug taken", body = ProblemResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
    )
)]
fn setup_run() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginBody,
    responses(
        (status = 200, description = "Logged in", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ProblemResponse),
    )
)]
fn login() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "auth",
    security(("fvoci_session" = [])),
    responses(
        (status = 204, description = "Logged out"),
    )
)]
fn logout() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "auth",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Current user", body = SessionUserOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn me_get() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/auth/me",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = PatchMeBody,
    responses(
        (status = 200, description = "Updated user", body = SessionUserOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
    )
)]
fn me_patch() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/workspaces",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Accessible workspaces", body = WorkspaceListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn list_my_workspaces() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/me/personal-workspace",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Personal workspace", body = WorkspaceMetaResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn personal_workspace() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    request_body = CreateWorkspaceBody,
    responses(
        (status = 201, description = "Created", body = WorkspaceMetaResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 409, description = "Slug taken", body = ProblemResponse),
    )
)]
fn create_workspace() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace metadata", body = WorkspaceMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_workspace() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = PatchWorkspaceBody,
    responses(
        (status = 200, description = "Updated workspace", body = WorkspaceMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn patch_workspace() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/members/{user_id}",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("user_id" = String, description = "Member user id"),
    ),
    request_body = MemberRoleBody,
    responses(
        (status = 200, description = "Updated member", body = MemberResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn patch_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/members/{user_id}",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("user_id" = String, description = "Member user id"),
    ),
    responses(
        (status = 200, description = "Removed member", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn remove_member() {}

#[cfg(feature = "api-schema")]
pub fn spec_json() -> String {
    ApiDoc::openapi().to_pretty_json().expect("openapi json")
}
