// Path stubs exist only for OpenAPI generation.

#[cfg(feature = "api-schema")]
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
#[cfg(feature = "api-schema")]
use utoipa::{Modify, OpenApi};

#[cfg(feature = "api-schema")]
use crate::api::dto::{
    AncestorsResponse, AttachmentOutput, AttachmentPartUrlResponse, AttachmentUploadedPartResponse,
    BodyResponse, BrandingOutput, CompleteAttachmentUploadBody, CreateAttachmentUploadBody,
    CreateAttachmentUploadResponse, CreateDocumentBody, CreateWorkspaceBody, DocumentMetaResponse,
    LoginBody, LoginResponse, MemberResponse, MemberRoleBody, OkResponse, PatchDocumentBody,
    PatchMeBody, PatchWorkspaceBody, ProblemResponse, PutAttachmentPartResponse,
    ResumeAttachmentUploadResponse, SessionUserOutput, SetupBody, SetupResponse,
    SetupStatusResponse, TreeResponse, WorkspaceListItemResponse, WorkspaceListResponse,
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
        description = "Rust slice HTTP contract for authentication, workspace, wiki document, and attachment operations."
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
        create_document,
        list_tree,
        get_document,
        patch_document,
        get_ancestors,
        get_body,
        create_attachment_upload,
        put_attachment_part,
        resume_attachment_upload,
        complete_attachment_upload,
        get_attachment_meta,
        download_attachment,
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
            CreateDocumentBody,
            PatchDocumentBody,
            DocumentMetaResponse,
            TreeResponse,
            AncestorsResponse,
            BodyResponse,
            CreateAttachmentUploadBody,
            CreateAttachmentUploadResponse,
            AttachmentPartUrlResponse,
            ResumeAttachmentUploadResponse,
            AttachmentUploadedPartResponse,
            CompleteAttachmentUploadBody,
            AttachmentOutput,
            PutAttachmentPartResponse,
            ProblemResponse,
        )
    ),
    modifiers(&CookieSecurityAddon),
    tags(
        (name = "setup", description = "Instance setup"),
        (name = "auth", description = "Authentication and profile"),
        (name = "workspaces", description = "Workspace membership and metadata"),
        (name = "documents", description = "Wiki documents"),
        (name = "attachments", description = "Wiki document attachments"),
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
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = CreateDocumentBody,
    responses(
        (status = 201, description = "Created", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
    )
)]
fn create_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tree",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Document tree", body = TreeResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_tree() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Document metadata", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = PatchDocumentBody,
    responses(
        (status = 200, description = "Updated metadata", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
    )
)]
fn patch_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/ancestors",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Ancestor chain", body = AncestorsResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_ancestors() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/body",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Document body", body = BodyResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_body() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = CreateAttachmentUploadBody,
    responses(
        (status = 201, description = "Upload session created", body = CreateAttachmentUploadResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "File too large", body = ProblemResponse),
    )
)]
fn create_attachment_upload() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/parts/{part_number}",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
        ("part_number" = i32, description = "Part number"),
    ),
    responses(
        (status = 200, description = "Part stored", body = PutAttachmentPartResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn put_attachment_part() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Resume state", body = ResumeAttachmentUploadResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn resume_attachment_upload() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    request_body = CompleteAttachmentUploadBody,
    responses(
        (status = 200, description = "Stored attachment", body = AttachmentOutput),
        (status = 400, description = "Invalid parts", body = ProblemResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn complete_attachment_upload() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Attachment metadata", body = AttachmentOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_attachment_meta() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Original bytes", content_type = "application/octet-stream"),
        (status = 206, description = "Partial content", content_type = "application/octet-stream"),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 416, description = "Range not satisfiable", body = ProblemResponse),
    )
)]
fn download_attachment() {}

#[cfg(feature = "api-schema")]
pub fn spec_json() -> String {
    ApiDoc::openapi().to_pretty_json().expect("openapi json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn generated_nullability_matches_runtime_contract() {
        let spec: Value = serde_json::from_str(&spec_json()).unwrap();
        let schemas = &spec["components"]["schemas"];
        for (name, fields) in [
            ("SessionUserOutput", &["familyName", "emailVerifiedAt"][..]),
            ("MemberResponse", &["familyName"][..]),
        ] {
            for field in fields {
                assert!(schemas[name]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(field)));
                assert!(schemas[name]["properties"][field]["type"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("null")));
            }
        }
        for (name, fields) in [
            (
                "DocumentMetaResponse",
                &["icon", "parentId", "projectId"][..],
            ),
            ("TreeNodeResponse", &["icon", "parentId", "projectId"][..]),
            ("AncestorResponse", &["icon", "projectId"][..]),
        ] {
            for field in fields {
                assert!(schemas[name]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(field)));
                assert!(schemas[name]["properties"][field]["type"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("null")));
            }
        }
        let create = &schemas["CreateDocumentBody"];
        assert!(create["required"]
            .as_array()
            .unwrap()
            .contains(&json!("parentId")));
        assert_eq!(create["properties"]["parentId"]["format"], "uuid");
        assert_eq!(schemas["PatchWorkspaceBody"]["required"], json!(["name"]));
        assert_eq!(
            schemas["PatchWorkspaceBody"]["properties"]["name"]["type"],
            "string"
        );
        for field in ["title", "status"] {
            assert_eq!(
                schemas["PatchDocumentBody"]["properties"][field]["type"],
                "string"
            );
        }
        let patch = &schemas["PatchMeBody"];
        assert_eq!(patch["required"], json!(["givenName"]));
        assert!(patch["properties"]["familyName"]["type"]
            .as_array()
            .unwrap()
            .contains(&json!("null")));
        for field in ["locale", "timezone", "weekStartsOn", "textScale"] {
            assert!(patch["properties"][field]["type"].is_string());
            let mut body = json!({"givenName":"A"});
            body.as_object_mut()
                .unwrap()
                .insert(field.into(), Value::Null);
            let error = crate::http::json_input::parse_patch_me(body)
                .err()
                .expect("null rejected");
            assert_eq!(error.source, Some(format!("/{field}")));
        }
    }
}
