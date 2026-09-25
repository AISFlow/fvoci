// Path stubs exist only for OpenAPI generation.

#[cfg(feature = "api-schema")]
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
#[cfg(feature = "api-schema")]
use utoipa::{Modify, OpenApi};

#[cfg(feature = "api-schema")]
use crate::api::dto::{
    AddProjectMemberBody, AncestorsResponse, ApiTokenCreateBody, ApiTokenCreatedOutput,
    ApiTokenListResponse, ApiTokenOutput, AttachmentOutput, AttachmentPartUrlResponse,
    AttachmentUploadedPartResponse, BodyResponse, BrandingOutput, CommentListResponse,
    CommentOutput, CommentReactionBody, CommentReactionSummary, CompleteAttachmentUploadBody,
    CreateAttachmentUploadBody, CreateAttachmentUploadResponse, CreateCommentBody,
    CreateDocumentBody, CreateGroupBody, CreateHolidayBody, CreateLabelBody, CreateMilestoneBody,
    CreateProjectBody, CreateTaskBody, CreateTaskDependencyBody, CreateWorkspaceBody,
    DocumentMetaResponse, ExpectedDatesBody, GroupListResponse, GroupMemberBody,
    GroupMemberListResponse, GroupMemberOutput, GroupOutput, HolidaysListResponse,
    IcsTokenResponse, InvitationAcceptBody, InvitationConsentItem, InvitationCreateBody,
    InvitationCreateResponse, InvitationLegalDocument, InvitationPublicResponse, LabelListResponse,
    LabelOutput, LoginBody, LoginResponse, LookupItemOutput, LookupListResponse,
    MeApiTokenCreateBody, MemberResponse, MemberRoleBody, MembersResponse, MilestoneListResponse,
    MilestoneOutput, MoveDocumentBody, MoveTaskBody, NotificationItemOutput,
    NotificationListResponse, NotificationPatchBody, NotificationPrefsBody,
    NotificationReadAllResponse, NotificationUnreadCountResponse, OkResponse, PasswordResetBody,
    PasswordResetConfirmBody, PatchCommentBody, PatchDocumentBody, PatchLabelBody, PatchMeBody,
    PatchMilestoneBody, PatchProjectBody, PatchTaskBody, PatchWorkspaceBody, ProblemResponse,
    ProjectGroupGrantBody, ProjectGroupGrantListResponse, ProjectGroupGrantOutput,
    ProjectGroupRevokeBody, ProjectListResponse, ProjectMembersResponse, ProjectOutput,
    PutAttachmentPartResponse, ResumeAttachmentUploadResponse, RevisionCreateResponse,
    RevisionDetailResponse, RevisionListResponse, RevisionMetaResponse, RevisionRestoreBody,
    RevisionRestoreResponse, SearchItemOutput, SearchListResponse, SearchSnippetPiece,
    SessionUserOutput, SetupBody, SetupResponse, SetupStatusResponse, SortDocumentBody,
    TaskChildOutput, TaskChildProgressOutput, TaskDependencyListResponse, TaskDependencyOutput,
    TaskListResponse, TaskMetaOutput, TaskOutput, TaskParentOutput, TrashItemResponse,
    TrashListResponse, TreeResponse, WorkflowOutput, WorkspaceListItemResponse,
    WorkspaceListResponse, WorkspaceMetaResponse,
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
        components.add_security_scheme(
            "bearer_api_token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("API token")
                    .build(),
            ),
        );
    }
}

#[cfg(feature = "api-schema")]
#[derive(OpenApi)]
#[openapi(
    info(
        title = "FVOCI API",
        version = "0.1.0",
        description = "Rust slice HTTP contract for authentication, workspace, wiki document, attachment, project, and task operations."
    ),
    paths(
        setup_status,
        setup_run,
        login,
        logout,
        me_get,
        me_patch,
        password_reset,
        confirm_password_reset,
        list_my_workspaces,
        personal_workspace,
        create_workspace,
        get_workspace,
        patch_workspace,
        list_members,
        patch_member,
        remove_member,
        create_invitation,
        get_invitation,
        accept_invitation,
        list_workspace_api_tokens,
        create_workspace_api_token,
        revoke_workspace_api_token,
        list_me_api_tokens,
        create_me_api_token,
        revoke_me_api_token,
        list_projects,
        create_project,
        get_project,
        patch_project,
        list_project_members,
        add_project_member,
        patch_project_member,
        delete_project_member,
        list_groups,
        create_group,
        purge_group,
        list_group_members,
        add_group_member,
        remove_group_member,
        list_project_group_grants,
        add_project_group_grant,
        remove_project_group_grant,
        list_document_group_grants,
        add_document_group_grant,
        remove_document_group_grant,
        get_project_workflow,
        lookup_display_id,
        workspace_search,
        list_tasks,
        create_task,
        get_task,
        patch_task,
        move_task,
        trash_task,
        restore_task,
        list_workspace_labels,
        list_project_labels,
        create_label,
        update_label,
        delete_label,
        list_project_milestones,
        create_milestone,
        update_milestone,
        delete_milestone,
        list_project_dependencies,
        add_task_dependency,
        remove_task_dependency,
        create_document,
        list_tree,
        get_document,
        patch_document,
        get_ancestors,
        get_body,
    create_revision, list_revisions, get_revision, restore_revision, move_document, sort_document, trash_document, restore_document, list_trash,
        create_attachment_upload,
        put_attachment_part,
        resume_attachment_upload,
        complete_attachment_upload,
        get_attachment_meta,
        download_attachment,
        list_document_comments,
        create_document_comment,
        list_task_comments,
        create_task_comment,
        patch_comment,
        delete_comment,
        resolve_comment,
        unresolve_comment,
        react_comment,
        list_workspace_notifications,
        unread_notification_count,
        patch_notification,
        read_all_notifications,
        get_notification_prefs,
        put_notification_prefs,
        list_me_notifications,
        list_workspace_holidays,
        create_workspace_holiday,
        delete_workspace_holiday,
        create_ics_token,
        get_ics_feed,
    ),
    components(
        schemas(
            SetupStatusResponse,
            BrandingOutput,
            SetupBody,
            SetupResponse,
            LoginBody,
            LoginResponse,
            PasswordResetBody,
            PasswordResetConfirmBody,
            SessionUserOutput,
            PatchMeBody,
            WorkspaceListResponse,
            WorkspaceListItemResponse,
            WorkspaceMetaResponse,
            CreateHolidayBody,
            HolidaysListResponse,
            IcsTokenResponse,
            OkResponse,
            MemberResponse,
            MembersResponse,
            InvitationCreateBody,
            InvitationCreateResponse,
            InvitationPublicResponse,
            ApiTokenCreateBody,
            MeApiTokenCreateBody,
            ApiTokenOutput,
            ApiTokenCreatedOutput,
            ApiTokenListResponse,
            InvitationLegalDocument,
            InvitationAcceptBody,
            InvitationConsentItem,
            CreateWorkspaceBody,
            PatchWorkspaceBody,
            MemberRoleBody,
            CreateProjectBody,
            PatchProjectBody,
            ProjectOutput,
            ProjectListResponse,
            ProjectMembersResponse,
            AddProjectMemberBody,
            CreateGroupBody,
            GroupOutput,
            GroupListResponse,
            GroupMemberBody,
            GroupMemberOutput,
            GroupMemberListResponse,
            ProjectGroupGrantBody,
            ProjectGroupRevokeBody,
            ProjectGroupGrantOutput,
            ProjectGroupGrantListResponse,
            WorkflowOutput,
            CreateTaskBody,
            PatchTaskBody,
            ExpectedDatesBody,
            MoveTaskBody,
            TaskMetaOutput,
            TaskOutput,
            TaskParentOutput,
            TaskChildOutput,
            TaskChildProgressOutput,
            TaskListResponse,
            LabelOutput,
            LabelListResponse,
            CreateLabelBody,
            PatchLabelBody,
            MilestoneOutput,
            MilestoneListResponse,
            CreateMilestoneBody,
            PatchMilestoneBody,
            TaskDependencyOutput,
            TaskDependencyListResponse,
            CreateTaskDependencyBody,
            LookupItemOutput,
            LookupListResponse,
            SearchSnippetPiece,
            SearchItemOutput,
            SearchListResponse,
            NotificationItemOutput,
            NotificationListResponse,
            NotificationUnreadCountResponse,
            NotificationPatchBody,
            NotificationReadAllResponse,
            NotificationPrefsBody,
            CreateDocumentBody,
            PatchDocumentBody,
            DocumentMetaResponse,
            TreeResponse,
            AncestorsResponse,
            BodyResponse,
    RevisionCreateResponse, RevisionMetaResponse, RevisionListResponse, RevisionDetailResponse, RevisionRestoreBody, RevisionRestoreResponse, MoveDocumentBody, SortDocumentBody, TrashListResponse, TrashItemResponse,
            CreateAttachmentUploadBody,
            CreateAttachmentUploadResponse,
            AttachmentPartUrlResponse,
            ResumeAttachmentUploadResponse,
            AttachmentUploadedPartResponse,
            CompleteAttachmentUploadBody,
            AttachmentOutput,
            PutAttachmentPartResponse,
            CreateCommentBody,
            PatchCommentBody,
            CommentReactionBody,
            CommentReactionSummary,
            CommentOutput,
            CommentListResponse,
            ProblemResponse,
        )
    ),
    modifiers(&CookieSecurityAddon),
    tags(
        (name = "setup", description = "Instance setup"),
        (name = "auth", description = "Authentication and profile"),
        (name = "workspaces", description = "Workspace membership and metadata"),
        (name = "projects", description = "Project and workflow management"),
        (name = "search", description = "Workspace search and display id lookup"),
        (name = "tasks", description = "Project task operations"),
        (name = "documents", description = "Wiki documents"),
        (name = "attachments", description = "Wiki document attachments"),
        (name = "comments", description = "Document and task comments"),
        (name = "notifications", description = "In-app notifications"),
        (name = "schedule", description = "Holidays and ICS calendar feeds"),
    )
)]
pub struct ApiDoc;

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size"),
    ),
    responses(
        (status = 200, description = "Document comments", body = CommentListResponse),
        (status = 400, description = "Invalid cursor", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_document_comments() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = CreateCommentBody,
    responses(
        (status = 201, description = "Created comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_document_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size"),
    ),
    responses(
        (status = 200, description = "Task comments", body = CommentListResponse),
        (status = 400, description = "Invalid cursor", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_task_comments() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = CreateCommentBody,
    responses(
        (status = 201, description = "Created comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_task_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/comments/{comment_id}",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("comment_id" = String, description = "Comment id"),
    ),
    request_body = PatchCommentBody,
    responses(
        (status = 200, description = "Updated comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn patch_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/comments/{comment_id}",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("comment_id" = String, description = "Comment id"),
    ),
    responses(
        (status = 200, description = "Deleted comment", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn delete_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("comment_id" = String, description = "Comment id"),
    ),
    responses(
        (status = 200, description = "Resolved comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn resolve_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("comment_id" = String, description = "Comment id"),
    ),
    responses(
        (status = 200, description = "Unresolved comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn unresolve_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions",
    tag = "comments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("comment_id" = String, description = "Comment id"),
    ),
    request_body = CommentReactionBody,
    responses(
        (status = 200, description = "Updated reactions", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Reaction conflict", body = ProblemResponse),
    )
)]
fn react_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/notifications",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("filter" = Option<String>, Query, description = "all, unread, or archived"),
        ("cursor" = Option<String>, Query, description = "Keyset cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-100"),
    ),
    responses(
        (status = 200, description = "Notification page", body = NotificationListResponse),
        (status = 400, description = "Invalid cursor or query", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_notifications() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/notifications/unread-count",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Unread count", body = NotificationUnreadCountResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn unread_notification_count() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/notifications/{id}",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Notification id"),
    ),
    request_body = NotificationPatchBody,
    responses(
        (status = 200, description = "Updated", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn patch_notification() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/notifications/read-all",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Marked read", body = NotificationReadAllResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn read_all_notifications() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/notification-prefs",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Notification prefs", body = NotificationPrefsBody),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_notification_prefs() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/notification-prefs",
    tag = "notifications",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = NotificationPrefsBody,
    responses(
        (status = 200, description = "Updated prefs", body = NotificationPrefsBody),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn put_notification_prefs() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/notifications",
    tag = "notifications",
    security(("fvoci_session" = [])),
    params(
        ("filter" = Option<String>, Query, description = "all, unread, or archived"),
        ("cursor" = Option<String>, Query, description = "Keyset cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-100"),
    ),
    responses(
        (status = 200, description = "Merged notification page", body = NotificationListResponse),
        (status = 400, description = "Invalid cursor or query", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn list_me_notifications() {}

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
    post,
    path = "/api/v1/auth/password-reset",
    tag = "auth",
    request_body = PasswordResetBody,
    responses(
        (status = 202, description = "Accepted", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn password_reset() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/password-reset/confirm",
    tag = "auth",
    request_body = PasswordResetConfirmBody,
    responses(
        (status = 200, description = "Password reset", body = OkResponse),
        (status = 400, description = "Invalid or expired token", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn confirm_password_reset() {}

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
    get,
    path = "/api/v1/workspaces/{workspace_id}/members",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace members", body = MembersResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_members() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/invitations",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = InvitationCreateBody,
    responses(
        (status = 201, description = "Invitation created", body = InvitationCreateResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Forbidden", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
    )
)]
fn create_invitation() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/invitations/{token}",
    tag = "workspaces",
    params(("token" = String, description = "Invitation token")),
    responses(
        (status = 200, description = "Invitation preview", body = InvitationPublicResponse),
        (status = 404, description = "Invitation not found or expired", body = ProblemResponse),
    )
)]
fn get_invitation() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/invitations/{token}/accept",
    tag = "workspaces",
    params(("token" = String, description = "Invitation token")),
    request_body = InvitationAcceptBody,
    responses(
        (status = 200, description = "Invitation accepted", body = LoginResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Cannot accept invitation", body = ProblemResponse),
        (status = 402, description = "Seat or guest limit", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
        (status = 410, description = "Expired or already accepted", body = ProblemResponse),
        (status = 428, description = "Consent required", body = ProblemResponse),
    )
)]
fn accept_invitation() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/api-tokens",
    tag = "api-tokens",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace API tokens", body = ApiTokenListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_api_tokens() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/api-tokens",
    tag = "api-tokens",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = ApiTokenCreateBody,
    responses(
        (status = 201, description = "Token created; secret shown once", body = ApiTokenCreatedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn create_workspace_api_token() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/api-tokens/{id}",
    tag = "api-tokens",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Token id"),
    ),
    responses(
        (status = 200, description = "Token revoked", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn revoke_workspace_api_token() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/api-tokens",
    tag = "api-tokens",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Current user API tokens", body = ApiTokenListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_me_api_tokens() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/me/api-tokens",
    tag = "api-tokens",
    security(("fvoci_session" = [])),
    request_body = MeApiTokenCreateBody,
    responses(
        (status = 201, description = "Token created; secret shown once", body = ApiTokenCreatedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn create_me_api_token() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/me/api-tokens/{id}",
    tag = "api-tokens",
    security(("fvoci_session" = [])),
    params(("id" = String, description = "Token id")),
    responses(
        (status = 200, description = "Token revoked", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn revoke_me_api_token() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Projects visible to caller", body = ProjectListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_projects() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = CreateProjectBody,
    responses(
        (status = 201, description = "Created project", body = ProjectOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Key taken", body = ProblemResponse),
    )
)]
fn create_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project metadata", body = ProjectOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = PatchProjectBody,
    responses(
        (status = 200, description = "Updated project", body = ProjectOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Lead invariant violated", body = ProblemResponse),
    )
)]
fn patch_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project members", body = ProjectMembersResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_members() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = AddProjectMemberBody,
    responses(
        (status = 201, description = "Added member", body = MemberResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Lead invariant violated", body = ProblemResponse),
    )
)]
fn add_project_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{user_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("user_id" = String, description = "Member user id"),
    ),
    request_body = MemberRoleBody,
    responses(
        (status = 200, description = "Updated member", body = MemberResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Lead invariant violated", body = ProblemResponse),
    )
)]
fn patch_project_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{user_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("user_id" = String, description = "Member user id"),
    ),
    responses(
        (status = 200, description = "Removed member", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Lead invariant violated", body = ProblemResponse),
    )
)]
fn delete_project_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/groups",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace groups", body = GroupListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_groups() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/groups",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = CreateGroupBody,
    responses(
        (status = 201, description = "Created group", body = GroupOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_group() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/groups/{group_id}",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("group_id" = String, description = "Group id"),
    ),
    responses(
        (status = 200, description = "Deleted group", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Last private-project lead", body = ProblemResponse),
    )
)]
fn purge_group() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/groups/{group_id}/members",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("group_id" = String, description = "Group id"),
    ),
    responses(
        (status = 200, description = "Group members", body = GroupMemberListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_group_members() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/groups/{group_id}/members",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("group_id" = String, description = "Group id"),
    ),
    request_body = GroupMemberBody,
    responses(
        (status = 201, description = "Added group member", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Already a member", body = ProblemResponse),
    )
)]
fn add_group_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/groups/{group_id}/members",
    tag = "groups",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("group_id" = String, description = "Group id"),
    ),
    request_body = GroupMemberBody,
    responses(
        (status = 200, description = "Removed group member", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn remove_group_member() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups",
    tag = "projects",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project group grants", body = ProjectGroupGrantListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_group_grants() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups",
    tag = "projects",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = ProjectGroupGrantBody,
    responses(
        (status = 201, description = "Granted project group", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Already granted", body = ProblemResponse),
    )
)]
fn add_project_group_grant() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups",
    tag = "projects",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = ProjectGroupRevokeBody,
    responses(
        (status = 200, description = "Revoked project group", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Last private-project lead", body = ProblemResponse),
    )
)]
fn remove_project_group_grant() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/groups",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Wiki document group grants", body = ProjectGroupGrantListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_document_group_grants() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/groups",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = ProjectGroupGrantBody,
    responses(
        (status = 201, description = "Granted wiki document group", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Already granted", body = ProblemResponse),
    )
)]
fn add_document_group_grant() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/groups",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = ProjectGroupRevokeBody,
    responses(
        (status = 200, description = "Revoked wiki document group", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn remove_document_group_grant() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project workflow", body = WorkflowOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_project_workflow() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("query" = Option<String>, Query, description = "JSON-encoded view query"),
        ("archived" = Option<String>, Query, description = "Filter archived tasks"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size"),
        ("from" = Option<String>, Query, description = "Schedule range start (YYYY-MM-DD)"),
        ("to" = Option<String>, Query, description = "Schedule range end (YYYY-MM-DD)"),
    ),
    responses(
        (status = 200, description = "Project tasks", body = TaskListResponse),
        (status = 400, description = "Invalid query", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_tasks() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/lookup/{display_id}",
    tag = "search",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("display_id" = String, description = "Display id such as LAB-1 or WIKI-2"),
        ("projectId" = Option<String>, Query, description = "Optional project scope filter"),
    ),
    responses(
        (status = 200, description = "Lookup matches", body = LookupListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not a workspace member", body = ProblemResponse),
    )
)]
fn lookup_display_id() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/search",
    tag = "search",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("q" = String, Query, description = "Search query"),
        ("type" = Option<String>, Query, description = "Result kind filter"),
        ("projectId" = Option<String>, Query, description = "Optional project scope"),
        ("tag" = Option<String>, Query, description = "Optional tag filter"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-50"),
        ("mode" = Option<String>, Query, description = "lexical or hybrid"),
    ),
    responses(
        (status = 200, description = "Search hits", body = SearchListResponse),
        (status = 400, description = "Invalid input or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not a workspace member", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Search unavailable", body = ProblemResponse),
    )
)]
fn workspace_search() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = CreateTaskBody,
    responses(
        (status = 201, description = "Created task", body = TaskMetaOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Task detail", body = TaskOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = PatchTaskBody,
    responses(
        (status = 200, description = "Updated task", body = TaskMetaOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Conflict", body = ProblemResponse),
    )
)]
fn patch_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = MoveTaskBody,
    responses(
        (status = 200, description = "Moved task", body = TaskMetaOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Conflict", body = ProblemResponse),
    )
)]
fn move_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Task trashed", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn trash_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/restore",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Task restored", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn restore_task() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/labels",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace labels", body = LabelListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_labels() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project labels", body = LabelListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_labels() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = CreateLabelBody,
    responses(
        (status = 201, description = "Created label", body = LabelOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_label() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("label_id" = String, description = "Label id"),
    ),
    request_body = PatchLabelBody,
    responses(
        (status = 200, description = "Label updated", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn update_label() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("label_id" = String, description = "Label id"),
    ),
    responses(
        (status = 200, description = "Label deleted", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn delete_label() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project milestones", body = MilestoneListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_milestones() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = CreateMilestoneBody,
    responses(
        (status = 201, description = "Created milestone", body = MilestoneOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_milestone() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("milestone_id" = String, description = "Milestone id"),
    ),
    request_body = PatchMilestoneBody,
    responses(
        (status = 200, description = "Milestone updated", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn update_milestone() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("milestone_id" = String, description = "Milestone id"),
    ),
    responses(
        (status = 200, description = "Milestone deleted", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn delete_milestone() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/dependencies",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project dependencies", body = TaskDependencyListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_dependencies() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = CreateTaskDependencyBody,
    responses(
        (status = 200, description = "Dependency added", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn add_task_dependency() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies/{blocked_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("blocked_id" = String, description = "Blocked task id"),
    ),
    responses(
        (status = 200, description = "Dependency removed", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn remove_task_dependency() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/holidays",
    tag = "schedule",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace holidays", body = HolidaysListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_holidays() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/holidays",
    tag = "schedule",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = CreateHolidayBody,
    responses(
        (status = 201, description = "Holiday added", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_workspace_holiday() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/holidays/{date}",
    tag = "schedule",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("date" = String, description = "Holiday date YYYY-MM-DD"),
    ),
    responses(
        (status = 200, description = "Holiday removed", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn delete_workspace_holiday() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/ics-token",
    tag = "schedule",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 201, description = "ICS feed URL", body = IcsTokenResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_ics_token() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/ics/{token}",
    tag = "schedule",
    params(("token" = String, description = "Hashed-at-rest feed token")),
    responses(
        (status = 200, description = "ICS calendar", content_type = "text/calendar"),
        (status = 404, description = "Not found or expired", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_ics_feed() {}

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
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/move",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = MoveDocumentBody,
    responses(
        (status = 200, description = "Moved document", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn move_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/sort",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = SortDocumentBody,
    responses(
        (status = 200, description = "Reordered document", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn sort_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/trash",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("children" = Option<String>, Query, description = "trash or reparent"),
    ),
    responses(
        (status = 200, description = "Trashed", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn trash_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/restore",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Restored", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Parent trashed", body = ProblemResponse),
    )
)]
fn restore_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/trash",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Trashed documents", body = TrashListResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_trash() {}

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
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 201, description = "Revision created", body = RevisionCreateResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collab unavailable", body = ProblemResponse),
    )
)]
fn create_revision() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("limit" = Option<i64>, Query, description = "Page size 1..=100, default 50"),
        ("cursor" = Option<String>, Query, description = "Opaque list cursor"),
    ),
    responses(
        (status = 200, description = "Revision list", body = RevisionListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_revisions() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("revision_id" = String, description = "Revision id"),
    ),
    responses(
        (status = 200, description = "Revision detail", body = RevisionDetailResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_revision() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("revision_id" = String, description = "Revision id"),
    ),
    request_body = RevisionRestoreBody,
    responses(
        (status = 200, description = "Restored", body = RevisionRestoreResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Restore rejected", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 504, description = "Collab timeout", body = ProblemResponse),
    )
)]
fn restore_revision() {}

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
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "File too large", body = ProblemResponse),
        (status = 429, description = "Create rate limited", body = ProblemResponse),
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
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Upload is not in the required state", body = ProblemResponse),
        (status = 413, description = "Part too large", body = ProblemResponse),
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
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Upload is not in the required state", body = ProblemResponse),
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
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Uploader mismatch", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Upload is not in the required state", body = ProblemResponse),
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
        (status = 401, description = "Authentication required", body = ProblemResponse),
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
        ("variant" = Option<String>, Query, description = "Omit for original bytes; preview is not stored in this slice"),
    ),
    responses(
        (status = 200, description = "Original bytes", content_type = "application/octet-stream"),
        (status = 206, description = "Partial content", content_type = "application/octet-stream"),
        (status = 400, description = "Invalid download variant", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
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

    fn schema_is_nullable(schema: &Value) -> bool {
        if schema["type"]
            .as_array()
            .is_some_and(|types| types.iter().any(|ty| ty == "null"))
        {
            return true;
        }
        for key in ["oneOf", "anyOf"] {
            let Some(alts) = schema[key].as_array() else {
                continue;
            };
            let has_null = alts
                .iter()
                .any(|alt| alt.get("type") == Some(&json!("null")));
            let has_value = alts
                .iter()
                .any(|alt| alt.get("type") != Some(&json!("null")));
            if has_null && has_value {
                return true;
            }
        }
        false
    }

    fn assert_required_nullable(schemas: &Value, name: &str, field: &str) {
        assert!(
            schemas[name]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} missing required"))
                .contains(&json!(field)),
            "{name}.{field} must be required"
        );
        assert!(
            schema_is_nullable(&schemas[name]["properties"][field]),
            "{name}.{field} must be nullable, got {}",
            schemas[name]["properties"][field]
        );
    }

    fn assert_required_non_nullable(schemas: &Value, name: &str, field: &str) {
        assert!(
            schemas[name]["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} missing required"))
                .contains(&json!(field)),
            "{name}.{field} must be required"
        );
        assert!(
            !schema_is_nullable(&schemas[name]["properties"][field]),
            "{name}.{field} must not be nullable, got {}",
            schemas[name]["properties"][field]
        );
    }

    #[test]
    fn generated_nullability_matches_runtime_contract() {
        let spec: Value = serde_json::from_str(&spec_json()).unwrap();
        let schemas = &spec["components"]["schemas"];
        for (name, fields) in [
            ("SessionUserOutput", &["familyName", "emailVerifiedAt"][..]),
            ("MemberResponse", &["familyName"][..]),
            ("ApiTokenOutput", &["userId", "expiresAt"][..]),
            ("ApiTokenCreatedOutput", &["userId", "expiresAt"][..]),
        ] {
            for field in fields {
                assert_required_nullable(schemas, name, field);
            }
        }
        for (name, fields) in [
            (
                "DocumentMetaResponse",
                &["icon", "parentId", "projectId"][..],
            ),
            ("TreeNodeResponse", &["icon", "parentId", "projectId"][..]),
            ("AncestorResponse", &["icon", "projectId"][..]),
            (
                "AttachmentOutput",
                &["sizeBytes", "completedAt", "preview"][..],
            ),
        ] {
            for field in fields {
                assert_required_nullable(schemas, name, field);
            }
        }
        for field in ["id", "name", "mime", "scanStatus"] {
            assert_required_non_nullable(schemas, "AttachmentOutput", field);
        }
        for field in ["id", "workspaceId", "name", "scopes", "createdAt"] {
            assert_required_non_nullable(schemas, "ApiTokenOutput", field);
            assert_required_non_nullable(schemas, "ApiTokenCreatedOutput", field);
        }
        assert_required_non_nullable(schemas, "ApiTokenCreatedOutput", "token");
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

    fn response_statuses(spec: &Value, path: &str, method: &str) -> Vec<String> {
        spec["paths"][path][method]["responses"]
            .as_object()
            .expect("responses")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn attachment_routes_declare_runtime_error_statuses() {
        let spec: Value = serde_json::from_str(&spec_json()).unwrap();
        let create = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads",
            "post",
        );
        for status in ["201", "400", "401", "404", "413", "429"] {
            assert!(
                create.iter().any(|s| s == status),
                "create missing {status}"
            );
        }
        let put = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/parts/{part_number}",
            "put",
        );
        for status in ["200", "400", "401", "403", "404", "409", "413"] {
            assert!(put.iter().any(|s| s == status), "put missing {status}");
        }
        let resume = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload",
            "get",
        );
        for status in ["200", "401", "403", "404", "409"] {
            assert!(
                resume.iter().any(|s| s == status),
                "resume missing {status}"
            );
        }
        let complete = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete",
            "post",
        );
        for status in ["200", "400", "401", "403", "404", "409"] {
            assert!(
                complete.iter().any(|s| s == status),
                "complete missing {status}"
            );
        }
        let meta = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}",
            "get",
        );
        for status in ["200", "401", "404"] {
            assert!(meta.iter().any(|s| s == status), "meta missing {status}");
        }
        let download = response_statuses(
            &spec,
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download",
            "get",
        );
        for status in ["200", "206", "400", "401", "404", "416"] {
            assert!(
                download.iter().any(|s| s == status),
                "download missing {status}"
            );
        }
    }
}
