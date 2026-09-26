// Path stubs exist only for OpenAPI generation.

#[cfg(feature = "api-schema")]
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
#[cfg(feature = "api-schema")]
use utoipa::{Modify, OpenApi};

#[cfg(feature = "api-schema")]
use crate::api::collections_dto::{
    CollectionAttachBody, CollectionCreateBody, CollectionFieldCreateBody,
    CollectionFieldListResponse, CollectionFieldOutput, CollectionFieldPatchBody,
    CollectionItemLookupResponse, CollectionItemOutput, CollectionListResponse,
    CollectionOptionOutput, CollectionOptionPatch, CollectionOutput, CollectionQueryBody,
    CollectionQueryDayOutput, CollectionQueryGroupOutput, CollectionQueryItemOutput,
    CollectionQueryPreviewOutput, CollectionQueryResponse, CollectionQueryWindow,
    CollectionValueBody, CollectionValueResponse, CollectionViewBody, CollectionViewListResponse,
    CollectionViewOutput, DocumentTagAssignBody, DocumentTagCreateBody, DocumentTagListResponse,
    DocumentTagOutput, DocumentTagPatchBody, DocumentTagPoolItemOutput,
    DocumentTagPoolListResponse, ProjectCollectionOutput, ProjectViewCreateBody,
    ProjectViewListResponse, ProjectViewOutput, ProjectViewPatchBody,
};
#[cfg(feature = "api-schema")]
use crate::api::dto::{
    ActivityActorOutput, ActivityChangeOutput, ActivityCommentParentOutput, ActivityItemOutput,
    ActivityListResponse, AddProjectMemberBody, AdminInstanceSettingsOutput, AdminSystemOutput,
    AdminUserItemOutput, AdminUserListResponse, AdminUserPatchBody, AdminUserPatchOutput,
    AdminWorkspaceItemOutput, AdminWorkspaceListResponse, AncestorsResponse, ApiTokenCreateBody,
    ApiTokenCreatedOutput, ApiTokenListResponse, ApiTokenOutput, AttachmentEditContextOutput,
    AttachmentListOutput, AttachmentOutput, AttachmentPartUrlResponse, AttachmentPreviewHtmlOutput,
    AttachmentUploadedPartResponse, AuditLogItemOutput, AuditLogListResponse, BodyResponse,
    BrandingOutput, BrandingPatchSchema, CloneProjectBody, CommentListResponse, CommentOutput,
    CommentReactionBody, CommentReactionSummary, CompleteAttachmentUploadBody, ConsentItemBody,
    ConsentsPendingResponse, ConsentsSubmitBody, CreateAttachmentUploadBody,
    CreateAttachmentUploadResponse, CreateCommentBody, CreateDocumentBody, CreateGroupBody,
    CreateHolidayBody, CreateLabelBody, CreateMilestoneBody, CreateProjectBody, CreateTaskBody,
    CreateTaskDependencyBody, CreateWorkspaceBody, DeleteWorkspaceBody, DocumentMetaResponse,
    DocumentShareLinkCreateBody, ExpectedDatesBody, GroupListResponse, GroupMemberBody,
    GroupMemberListResponse, GroupMemberOutput, GroupOutput, HolidaysListResponse,
    IcsTokenResponse, ImportJobResponse, InstanceAdminBody, InstanceSettingsOutput,
    InstanceSettingsPatchSchema, InvitationAcceptBody, InvitationConsentItem, InvitationCreateBody,
    InvitationCreateResponse, InvitationLegalDocument, InvitationPublicResponse, LabelListResponse,
    LabelOutput, LegalDocumentOutput, LegalPublishBody, LegalVersionMetaOutput,
    LegalVersionsResponse, LoginBody, LoginResponse, LookupItemOutput, LookupListResponse,
    MeApiTokenCreateBody, MemberConsentOutput, MemberResponse, MemberRoleBody, MembersResponse,
    MilestoneListResponse, MilestoneOutput, MoveDocumentBody, MoveTaskBody, NotificationItemOutput,
    NotificationListResponse, NotificationPatchBody, NotificationPrefsBody,
    NotificationReadAllResponse, NotificationUnreadCountResponse, OkResponse, PasswordResetBody,
    PasswordResetConfirmBody, PatchCommentBody, PatchDocumentBody, PatchLabelBody, PatchMeBody,
    PatchMilestoneBody, PatchProjectBody, PatchTaskBody, PatchWorkspaceBody, ProblemResponse,
    ProjectGroupGrantBody, ProjectGroupGrantListResponse, ProjectGroupGrantOutput,
    ProjectGroupRevokeBody, ProjectListResponse, ProjectMembersResponse, ProjectOutput,
    PublicBrandingOutput, PublicSettingsValues, PutAttachmentPartResponse, RecentItemOutput,
    RecentListResponse, ResumeAttachmentUploadResponse, RevisionCreateResponse,
    RevisionDetailResponse, RevisionListResponse, RevisionMetaResponse, RevisionRestoreBody,
    RevisionRestoreResponse, SearchItemOutput, SearchListResponse, SearchSnippetPiece,
    SessionUserOutput, SetupBody, SetupResponse, SetupStatusResponse, ShareCreateBody,
    ShareLinkCreatedOutput, ShareLinkListResponse, ShareLinkOutput, SharePublicMetaOutput,
    SortDocumentBody, StarCreateBody, StarItemOutput, StarListResponse, StartImportBody,
    TaskChildOutput, TaskChildProgressOutput, TaskDependencyListResponse, TaskDependencyOutput,
    TaskListResponse, TaskMetaOutput, TaskOutput, TaskParentOutput, TrashItemResponse,
    TrashListResponse, TreeResponse, WorkflowOutput, WorkspaceConsentsResponse,
    WorkspaceListItemResponse, WorkspaceListResponse, WorkspaceMemberConsentsOutput,
    WorkspaceMetaResponse,
};
#[cfg(feature = "api-schema")]
use crate::api::dto::{
    AiDocumentBody, AiGenerateTasksOutput, AiSuggestLinksOutput, AiSummarizeOutput,
    GithubInstallOutput, GithubInstallUrlOutput, GithubIssueLinkBody, GithubIssueLinkOutput,
    WebhookCreateBody, WebhookCreatedOutput, WebhookListResponse, WebhookOutput,
};
#[cfg(feature = "api-schema")]
use crate::api::dto::{
    DashboardProjectOutput, DashboardRecentItemOutput, DashboardWorkspaceOutput, EmailChangeBody,
    ErasureScheduleOutput, IdentitiesOutput, IdentityOutput, MagicLinkBody, MeDashboardResponse,
    MeLocateResponse, PasswordChangeBody, ProviderOutput, ProvidersOutput, TokenBody, WithdrawBody,
    WorkspaceStatusOutput,
};
#[cfg(feature = "api-schema")]
use crate::settings::catalog::{
    AttachmentPreviewSettings, AuthSettings, BrandingAsset, BrandingSettings, DefaultsUserSettings,
    EmbedSettings, FeaturesSettings, I18nSettings, OperatorSettings, SecuritySettings,
    SettingsValues, SharePolicy,
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
        withdraw,
        cancel_withdraw,
        email_change,
        email_confirm,
        password_change,
        magic_link,
        magic_link_consume,
        auth_providers,
        auth_identities,
        me_export,
        me_dashboard,
        me_locate,
        list_my_workspaces,
        personal_workspace,
        create_workspace,
        get_workspace,
        patch_workspace,
        delete_workspace,
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
        clone_project,
        get_project,
        patch_project,
        list_project_documents,
        create_project_document,
        get_project_document,
        patch_project_document,
        move_project_document,
        get_project_document_body,
        delete_project,
        archive_project,
        unarchive_project,
        restore_project,
        delete_project_document,
        trash_project_document,
        restore_project_document,
        sort_project_document,
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
        global_search,
        workspace_search,
        list_tasks,
        create_task,
        get_task,
        list_task_activity,
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
        export_markdown,
        export_pdf,
        export_docx,
        export_pptx,
        export_project_markdown,
        export_project_pdf,
        export_project_docx,
        export_project_pptx,
        start_import,
        get_import_status,
    create_revision, list_revisions, get_revision, restore_revision, move_document, sort_document, trash_document, restore_document, list_trash,
        create_attachment_upload,
        put_attachment_part,
        resume_attachment_upload,
        complete_attachment_upload,
        get_attachment_meta,
        delete_attachment,
        download_attachment,
        create_project_document_attachment_upload,
        create_task_attachment_upload,
        list_task_attachments,
        get_attachment_edit_context,
        get_attachment_preview_html,
        create_attachment_edit_copy,
        list_document_comments,
        create_document_comment,
        list_project_document_comments,
        create_project_document_comment,
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
        list_webhooks_path,
        create_webhook_path,
        remove_webhook_path,
        get_github_path,
        remove_github_path,
        install_github_path,
        link_github_issue_path,
        github_callback_path,
        github_webhook_path,
        ai_summarize_path,
        ai_generate_tasks_path,
        ai_suggest_links_path,
        list_stars,
        create_star,
        delete_star,
        list_recent,
        list_share_links,
        create_share_link,
        revoke_share_link,
        list_wiki_document_share_links,
        create_wiki_document_share_link,
        list_project_document_share_links,
        create_project_document_share_link,
        get_share_meta,
        get_share_body,
        get_share_tree,
        get_share_pdf,
        search_share,
        get_share_document,
        get_share_attachment,
        download_share_attachment,
        admin_audit,
        admin_system,
        admin_users,
        admin_update_users,
        admin_workspaces,
        admin_instance_settings,
        admin_update_instance_settings,
        admin_instance_admins,
        admin_publish_legal,
        admin_upload_branding_asset,
        admin_remove_branding_asset,
        branding_asset,
        instance_settings_public,
        legal_get,
        legal_versions,
        pending_consents,
        submit_consents,
        workspace_consents,
        list_document_tags,
        create_document_tag,
        update_document_tag,
        delete_document_tag,
        list_wiki_document_tags,
        assign_wiki_document_tag,
        unassign_wiki_document_tag,
        list_project_document_tags,
        assign_project_document_tag,
        unassign_project_document_tag,
        list_collections,
        create_collection,
        list_collection_fields,
        create_collection_field,
        update_collection_field,
        attach_collection_item,
        put_collection_value,
        query_collection,
        list_collection_views,
        create_collection_view,
        update_collection_view,
        delete_collection_view,
        get_document_collection_item,
        get_task_collection_item,
        get_project_collection,
        list_project_views,
        create_project_view,
        update_project_view,
        delete_project_view,
    ),
    components(
        schemas(
            WebhookCreateBody,
            WebhookOutput,
            WebhookCreatedOutput,
            WebhookListResponse,
            GithubInstallOutput,
            GithubInstallUrlOutput,
            GithubIssueLinkBody,
            GithubIssueLinkOutput,
            AiDocumentBody,
            AiSummarizeOutput,
            AiGenerateTasksOutput,
            AiSuggestLinksOutput,
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
            DeleteWorkspaceBody,
            MemberRoleBody,
            CreateProjectBody,
            CloneProjectBody,
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
            StartImportBody,
            ImportJobResponse,
            RevisionCreateResponse, RevisionMetaResponse, RevisionListResponse, RevisionDetailResponse, RevisionRestoreBody, RevisionRestoreResponse, MoveDocumentBody, SortDocumentBody, TrashListResponse, TrashItemResponse,
            CreateAttachmentUploadBody,
            CreateAttachmentUploadResponse,
            AttachmentPartUrlResponse,
            ResumeAttachmentUploadResponse,
            AttachmentUploadedPartResponse,
            CompleteAttachmentUploadBody,
            AttachmentOutput,
            AttachmentListOutput,
            AttachmentEditContextOutput,
            AttachmentPreviewHtmlOutput,
            PutAttachmentPartResponse,
            CreateCommentBody,
            PatchCommentBody,
            CommentReactionBody,
            CommentReactionSummary,
            CommentOutput,
            ActivityActorOutput,
            ActivityChangeOutput,
            ActivityCommentParentOutput,
            ActivityItemOutput,
            ActivityListResponse,
            CommentListResponse,
    DashboardProjectOutput, DashboardRecentItemOutput, DashboardWorkspaceOutput, DocumentShareLinkCreateBody, EmailChangeBody, ErasureScheduleOutput, IdentitiesOutput, IdentityOutput, MagicLinkBody, MeDashboardResponse, MeLocateResponse, PasswordChangeBody, ProviderOutput, ProvidersOutput, RecentItemOutput, RecentListResponse, ShareCreateBody, ShareLinkCreatedOutput, ShareLinkListResponse, ShareLinkOutput, SharePublicMetaOutput, StarCreateBody, StarItemOutput, StarListResponse, TokenBody, WithdrawBody, WorkspaceStatusOutput,
            ProblemResponse,
            AuditLogItemOutput,
            AuditLogListResponse,
            AdminSystemOutput,
            AdminUserItemOutput,
            AdminUserListResponse,
            AdminUserPatchBody,
            AdminUserPatchOutput,
            AdminWorkspaceItemOutput,
            AdminWorkspaceListResponse,
            InstanceAdminBody,
            LegalPublishBody,
            LegalDocumentOutput,
            LegalVersionMetaOutput,
            LegalVersionsResponse,
            ConsentItemBody,
            ConsentsSubmitBody,
            ConsentsPendingResponse,
            MemberConsentOutput,
            WorkspaceMemberConsentsOutput,
            WorkspaceConsentsResponse,
            AdminInstanceSettingsOutput,
            PublicBrandingOutput,
            PublicSettingsValues,
            InstanceSettingsOutput,
            InstanceSettingsPatchSchema,
            BrandingPatchSchema,
            SettingsValues,
            BrandingSettings,
            BrandingAsset,
            DefaultsUserSettings,
            AuthSettings,
            SharePolicy,
            EmbedSettings,
            FeaturesSettings,
            AttachmentPreviewSettings,
            I18nSettings,
            SecuritySettings,
            OperatorSettings,
            DocumentTagOutput,
            DocumentTagPoolItemOutput,
            DocumentTagPoolListResponse,
            DocumentTagListResponse,
            DocumentTagCreateBody,
            DocumentTagPatchBody,
            DocumentTagAssignBody,
            CollectionOutput,
            CollectionListResponse,
            ProjectCollectionOutput,
            CollectionOptionOutput,
            CollectionFieldOutput,
            CollectionFieldListResponse,
            CollectionItemOutput,
            CollectionItemLookupResponse,
            CollectionValueResponse,
            CollectionQueryItemOutput,
            CollectionQueryPreviewOutput,
            CollectionQueryGroupOutput,
            CollectionQueryDayOutput,
            CollectionQueryResponse,
            CollectionViewOutput,
            CollectionViewListResponse,
            ProjectViewOutput,
            ProjectViewListResponse,
            CollectionCreateBody,
            CollectionFieldCreateBody,
            CollectionOptionPatch,
            CollectionFieldPatchBody,
            CollectionAttachBody,
            CollectionValueBody,
            CollectionQueryWindow,
            CollectionQueryBody,
            CollectionViewBody,
            ProjectViewCreateBody,
            ProjectViewPatchBody,
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
        (name = "attachments", description = "Document and task attachments"),
        (name = "comments", description = "Document and task comments"),
        (name = "notifications", description = "In-app notifications"),
        (name = "schedule", description = "Holidays and ICS calendar feeds"),
        (name = "integrations", description = "Webhooks, GitHub App and document AI actions"),
        (name = "document-tags", description = "Workspace document tags"),
        (name = "collections", description = "Typed collections, values, queries and views"),
    )
)]
pub struct ApiDoc;

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments",
    tag = "comments",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments",
    tag = "comments",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size"),
    ),
    responses(
        (status = 200, description = "Project document comments", body = CommentListResponse),
        (status = 400, description = "Invalid cursor", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_document_comments() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments",
    tag = "comments",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = CreateCommentBody,
    responses(
        (status = 201, description = "Created comment", body = CommentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_project_document_comment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments",
    tag = "comments",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
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
    post,
    path = "/api/v1/auth/withdraw",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = WithdrawBody,
    responses(
        (status = 200, description = "Erasure scheduled; session cookie cleared", body = ErasureScheduleOutput),
        (status = 400, description = "confirm_invalid", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 409, description = "owner_transfer_required or last_instance_admin", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn withdraw() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/cancel-withdraw",
    tag = "auth",
    request_body = TokenBody,
    responses(
        (status = 200, description = "Withdrawal cancelled", body = OkResponse),
        (status = 404, description = "Unknown, used or expired cancel token", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn cancel_withdraw() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/auth/email",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = EmailChangeBody,
    responses(
        (status = 202, description = "Accepted (same response whether or not mail was sent)", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn email_change() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/email/confirm",
    tag = "auth",
    request_body = TokenBody,
    responses(
        (status = 200, description = "Email changed", body = OkResponse),
        (status = 400, description = "magic_invalid", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn email_confirm() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/auth/password",
    tag = "auth",
    security(("fvoci_session" = [])),
    request_body = PasswordChangeBody,
    responses(
        (status = 200, description = "Password changed; other sessions revoked", body = OkResponse),
        (status = 400, description = "password_invalid", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn password_change() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/magic-link",
    tag = "auth",
    request_body = MagicLinkBody,
    responses(
        (status = 202, description = "Accepted (same response for unknown addresses)", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn magic_link() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/magic-link/consume",
    tag = "auth",
    request_body = TokenBody,
    responses(
        (status = 200, description = "Signed in", body = LoginResponse),
        (status = 400, description = "magic_invalid", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn magic_link_consume() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/auth/providers",
    tag = "auth",
    responses(
        (status = 200, description = "Sign-in methods", body = ProvidersOutput),
    )
)]
fn auth_providers() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/auth/identities",
    tag = "auth",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Linked external identities", body = IdentitiesOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn auth_identities() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/export",
    tag = "auth",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "ZIP: profile.json, comments.json, attachments.json, attachments/*", content_type = "application/zip"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn me_export() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/dashboard",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(("lastVisited" = Option<String>, Query, description = "Workspace id to list first")),
    responses(
        (status = 200, description = "Cross-workspace dashboard", body = MeDashboardResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn me_dashboard() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/me/locate",
    tag = "workspaces",
    security(("fvoci_session" = [])),
    params(
        ("type" = String, Query, description = "task or document"),
        ("id" = String, Query, description = "Task or document id"),
    ),
    responses(
        (status = 200, description = "Workspace holding the target", body = MeLocateResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not visible", body = ProblemResponse),
    )
)]
fn me_locate() {}

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
    delete,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = DeleteWorkspaceBody,
    responses(
        (status = 200, description = "Workspace deleted", body = OkResponse),
        (status = 400, description = "Invalid confirmation slug", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Insufficient permissions", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Personal workspace is immutable", body = ProblemResponse),
    )
)]
fn delete_workspace() {}

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
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("deleted" = Option<String>, Query, description = "true lists restorable deleted projects (workspace admins)"),
    ),
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
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/clone",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Source project id"),
    ),
    request_body = CloneProjectBody,
    responses(
        (status = 201, description = "Cloned project", body = ProjectOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Key taken", body = ProblemResponse),
    )
)]
fn clone_project() {}

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
    path = "/api/v1/search",
    tag = "search",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read", "tasks.read"])),
    params(
        ("q" = String, Query, description = "Search query"),
        ("type" = Option<String>, Query, description = "Result kind filter"),
        ("tag" = Option<String>, Query, description = "Optional tag filter"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-50"),
        ("mode" = Option<String>, Query, description = "lexical or hybrid; global search stays lexical"),
    ),
    responses(
        (status = 200, description = "Cross-workspace search hits", body = SearchListResponse),
        (status = 400, description = "Invalid input or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Search unavailable", body = ProblemResponse),
    )
)]
fn global_search() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/search",
    tag = "search",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read", "tasks.read"])),
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
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("filter" = Option<String>, Query, description = "all | comments | changes"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size"),
    ),
    responses(
        (status = 200, description = "Task activity feed", body = ActivityListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_task_activity() {}

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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project archived", body = ProblemResponse),
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
        (status = 409, description = "Project or task archived", body = ProblemResponse),
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
        (status = 409, description = "Project or task archived", body = ProblemResponse),
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
    get,
    path = "/api/v1/workspaces/{workspace_id}/stars",
    tag = "stars",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Starred documents and tasks the actor can still read", body = StarListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_stars() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/stars",
    tag = "stars",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = StarCreateBody,
    responses(
        (status = 201, description = "Star (idempotent)", body = StarItemOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Target not found or not readable", body = ProblemResponse),
    )
)]
fn create_star() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/stars/{id}",
    tag = "stars",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Star id"),
    ),
    responses(
        (status = 200, description = "Star removed", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
    )
)]
fn delete_star() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/recent",
    tag = "stars",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("limit" = Option<i32>, Query, description = "1..=50, default 20"),
    ),
    responses(
        (status = 200, description = "Recently updated readable documents and tasks", body = RecentListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_recent() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "All links for owners/admins, otherwise the actor's own", body = ShareLinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_share_links() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = ShareCreateBody,
    responses(
        (status = 201, description = "Share link with its one-time URL", body = ShareLinkCreatedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no edit permission", body = ProblemResponse),
    )
)]
fn create_share_link() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/share-links/{id}",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Share link id"),
    ),
    responses(
        (status = 200, description = "Revoked", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn revoke_share_link() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Wiki document id"),
    ),
    responses(
        (status = 200, description = "Links of this document visible to the actor", body = ShareLinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_wiki_document_share_links() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("id" = String, description = "Wiki document id"),
    ),
    request_body = DocumentShareLinkCreateBody,
    responses(
        (status = 201, description = "Share link with its one-time URL", body = ShareLinkCreatedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no edit permission", body = ProblemResponse),
    )
)]
fn create_wiki_document_share_link() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("id" = String, description = "Project document id"),
    ),
    responses(
        (status = 200, description = "Links of this document visible to the actor", body = ShareLinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_document_share_links() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
    tag = "share",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("id" = String, description = "Project document id"),
    ),
    request_body = DocumentShareLinkCreateBody,
    responses(
        (status = 201, description = "Share link with its one-time URL", body = ShareLinkCreatedOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no edit permission", body = ProblemResponse),
    )
)]
fn create_project_document_share_link() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}",
    tag = "share",
    params(("token" = String, description = "Share token")),
    responses(
        (status = 200, description = "Public share metadata", body = SharePublicMetaOutput),
        (status = 404, description = "Unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_meta() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/body",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("format" = Option<String>, Query, description = "html (default) | fragment | md"),
    ),
    responses(
        (status = 200, description = "Shared root document body", content_type = "text/html"),
        (status = 304, description = "Not modified (fragment/md)"),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_body() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/tree",
    tag = "share",
    params(("token" = String, description = "Share token")),
    responses(
        (status = 200, description = "Visible subtree", body = TreeResponse),
        (status = 404, description = "Unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_tree() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/pdf",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("documentId" = Option<String>, Query, description = "Document inside the share"),
    ),
    responses(
        (status = 200, description = "PDF of the shared root or of `documentId` in the share", content_type = "application/pdf"),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Outside the share, unknown, expired or revoked", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_pdf() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/search",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("q" = String, Query, description = "1..=200 characters"),
    ),
    responses(
        (status = 200, description = "Hits inside the share", body = SearchListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Search unavailable", body = ProblemResponse),
    )
)]
fn search_share() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/documents/{document_id}",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("document_id" = String, description = "Document inside the share"),
        ("format" = Option<String>, Query, description = "html (default) | fragment | md"),
    ),
    responses(
        (status = 200, description = "Document body", content_type = "text/html"),
        (status = 304, description = "Not modified (fragment/md)"),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Outside the share, unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/attachments/{attachment_id}",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Attachment metadata", body = AttachmentOutput),
        (status = 404, description = "Outside the share, unknown, expired or revoked", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn get_share_attachment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/share/{token}/attachments/{attachment_id}/download",
    tag = "share",
    params(
        ("token" = String, description = "Share token"),
        ("attachment_id" = String, description = "Attachment id"),
        ("variant" = Option<String>, Query, description = "Omit for original bytes; `preview` for the published WebP preview"),
    ),
    responses(
        (status = 200, description = "Original bytes, or the WebP preview for variant=preview", content_type = "application/octet-stream"),
        (status = 304, description = "Preview not modified (If-None-Match)"),
        (status = 400, description = "Invalid download variant", body = ProblemResponse),
        (status = 404, description = "Outside the share, unknown, expired or revoked, or no preview", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn download_share_attachment() {}

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
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project document tree", body = TreeResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_project_documents() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = CreateDocumentBody,
    responses(
        (status = 201, description = "Created project document", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn create_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Project document metadata", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = PatchDocumentBody,
    responses(
        (status = 200, description = "Updated project document", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn patch_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/move",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = MoveDocumentBody,
    responses(
        (status = 200, description = "Moved project document", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn move_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("format" = Option<String>, Query, description = "`md` answers Markdown (contentMd)"),
    ),
    responses(
        (status = 200, description = "Project document body (JSON or Markdown)", body = crate::api::documents_dto::DocumentBodyResponse),
        (status = 400, description = "Invalid format", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_project_document_body() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project and its documents moved to trash", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn delete_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Archived", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn archive_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/unarchive",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Unarchived", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn unarchive_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Restored project", body = ProjectOutput),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn restore_project() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("children" = Option<String>, Query, description = "trash or reparent"),
    ),
    responses(
        (status = 200, description = "Trashed", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Project root document", body = ProblemResponse),
    )
)]
fn delete_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/trash",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("children" = Option<String>, Query, description = "trash or reparent"),
    ),
    responses(
        (status = 200, description = "Trashed", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Project root document", body = ProblemResponse),
    )
)]
fn trash_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/restore",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Restored", body = OkResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Parent trashed", body = ProblemResponse),
    )
)]
fn restore_project_document() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/sort",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = SortDocumentBody,
    responses(
        (status = 200, description = "Reordered project document", body = DocumentMetaResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn sort_project_document() {}

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
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("format" = Option<String>, Query, description = "`md` answers Markdown (contentMd)"),
    ),
    responses(
        (status = 200, description = "Document body (JSON or Markdown)", body = crate::api::documents_dto::DocumentBodyResponse),
        (status = 400, description = "Invalid format", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_body() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/md",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Markdown export", content_type = "text/markdown"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_markdown() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/pdf",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "PDF export", content_type = "application/pdf"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_pdf() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/docx",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "DOCX export", content_type = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_docx() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/pptx",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "PPTX export", content_type = "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_pptx() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/md",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Markdown export", content_type = "text/markdown"),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_project_markdown() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pdf",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "PDF export", content_type = "application/pdf"),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_project_pdf() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/docx",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "DOCX export", content_type = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_project_docx() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pptx",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "PPTX export", content_type = "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        (status = 400, description = "Stored body is not a valid document", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Export exceeds document max body bytes", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn export_project_pptx() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/import",
    tag = "import",
    security(("fvoci_session" = [])),
    request_body = StartImportBody,
    responses(
        (status = 201, description = "Import job created (markdown-zip: completed; office-file, notion-zip: running)", body = ImportJobResponse),
        (status = 400, description = "invalid_input, or import_failed", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found, not a workspace admin, or bearer token", body = ProblemResponse),
        (status = 413, description = "Body or decoded file exceeds the import limit", body = ProblemResponse),
        (status = 415, description = "Body is not application/json", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
    )
)]
fn start_import() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/import/{import_job_id}",
    tag = "import",
    security(("fvoci_session" = [])),
    params(
        ("import_job_id" = String, description = "Import job id"),
        ("workspace_id" = String, Query, description = "Workspace id"),
    ),
    responses(
        (status = 200, description = "Import job status", body = ImportJobResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_import_status() {}

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
        (status = 402, description = "Storage or upload limit", body = ProblemResponse),
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
        ("variant" = Option<String>, Query, description = "Omit for original bytes; `preview` for the published WebP preview (image/webp, inline, ETag)"),
    ),
    responses(
        (status = 200, description = "Original bytes, or the WebP preview for variant=preview", content_type = "application/octet-stream"),
        (status = 206, description = "Partial content", content_type = "application/octet-stream"),
        (status = 304, description = "Preview not modified (If-None-Match)"),
        (status = 400, description = "Invalid download variant", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 416, description = "Range not satisfiable", body = ProblemResponse),
    )
)]
fn download_attachment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Deleted; storage is reclaimed through the object journal", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden (uploader needs edit, others manage)", body = ProblemResponse),
        (status = 409, description = "Parent project or task archived", body = ProblemResponse),
    )
)]
fn delete_attachment() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/uploads",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Project document id"),
    ),
    request_body = CreateAttachmentUploadBody,
    responses(
        (status = 201, description = "Upload session created", body = CreateAttachmentUploadResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 402, description = "Storage or upload limit", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 413, description = "File too large", body = ProblemResponse),
        (status = 429, description = "Create rate limited", body = ProblemResponse),
    )
)]
fn create_project_document_attachment_upload() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/uploads",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = CreateAttachmentUploadBody,
    responses(
        (status = 201, description = "Upload session created", body = CreateAttachmentUploadResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 402, description = "Storage or upload limit", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Project or task archived", body = ProblemResponse),
        (status = 413, description = "File too large", body = ProblemResponse),
        (status = 429, description = "Create rate limited", body = ProblemResponse),
    )
)]
fn create_task_attachment_upload() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/attachments",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Task attachments, oldest first", body = AttachmentListOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_task_attachments() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/edit-context",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Whether the caller may save an edited HWP/HWPX copy", body = AttachmentEditContextOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Attachment failed virus scan", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_attachment_edit_context() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/edit-copy",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Source HWP/HWPX attachment id"),
    ),
    request_body = CreateAttachmentUploadBody,
    responses(
        (status = 201, description = "Upload session for the edited copy on the same parent", body = CreateAttachmentUploadResponse),
        (status = 400, description = "Invalid input or source is not HWP/HWPX", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 402, description = "Storage or upload limit", body = ProblemResponse),
        (status = 403, description = "Attachment failed virus scan", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Parent archived", body = ProblemResponse),
        (status = 413, description = "File too large", body = ProblemResponse),
        (status = 429, description = "Create rate limited", body = ProblemResponse),
    )
)]
fn create_attachment_edit_copy() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/preview-html",
    tag = "attachments",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("attachment_id" = String, description = "Attachment id"),
    ),
    responses(
        (status = 200, description = "Extracted text of an office (or, in server mode, HWP/HWPX) attachment as one escaped <pre>", body = AttachmentPreviewHtmlOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Attachment failed virus scan", body = ProblemResponse),
        (status = 404, description = "Not found, forbidden, or not served in the current attachmentPreview mode", body = ProblemResponse),
        (status = 413, description = "preview_not_available: no extracted text", body = ProblemResponse),
        (status = 429, description = "Rate limited (60/min)", body = ProblemResponse),
    )
)]
fn get_attachment_preview_html() {}

#[cfg(feature = "api-schema")]
pub fn spec_json() -> String {
    let mut doc = ApiDoc::openapi();
    doc.merge(crate::api::openapi_identity::IdentityApiDoc::openapi());
    doc.merge(crate::api::openapi_documents::DocumentsApiDoc::openapi());
    doc.to_pretty_json().expect("openapi json")
}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/admin/audit",
    tag = "admin",
    security(("fvoci_session" = [])),
    params(
        ("limit" = Option<i32>, Query, description = "Page size 1-100 (default 50)"),
        ("cursor" = Option<String>, Query, description = "Keyset cursor"),
    ),
    responses(
        (status = 200, description = "Instance audit log, newest first", body = AuditLogListResponse),
        (status = 400, description = "Invalid query or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
        (status = 428, description = "Consent required", body = ProblemResponse),
    )
)]
fn admin_audit() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/admin/system",
    tag = "admin",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Instance counts", body = AdminSystemOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_system() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    tag = "admin",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Instance users", body = AdminUserListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_users() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/admin/users",
    tag = "admin",
    security(("fvoci_session" = [])),
    request_body = AdminUserPatchBody,
    responses(
        (status = 200, description = "User updated", body = AdminUserPatchOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 402, description = "Seat limit", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin or user not found", body = ProblemResponse),
        (status = 409, description = "last_instance_admin or self_suspension", body = ProblemResponse),
    )
)]
fn admin_update_users() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/admin/workspaces",
    tag = "admin",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Live workspaces", body = AdminWorkspaceListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_workspaces() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/admin/instance-settings",
    tag = "admin",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "All settings with override state", body = AdminInstanceSettingsOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_instance_settings() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/admin/instance-settings",
    tag = "admin",
    security(("fvoci_session" = [])),
    request_body = InstanceSettingsPatchSchema,
    responses(
        (status = 200, description = "Settings after the change", body = AdminInstanceSettingsOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_update_instance_settings() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/admin/instance-admins",
    tag = "admin",
    security(("fvoci_session" = [])),
    request_body = InstanceAdminBody,
    responses(
        (status = 200, description = "Flag set", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 402, description = "Seat limit", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin or user not found", body = ProblemResponse),
        (status = 409, description = "last_instance_admin", body = ProblemResponse),
    )
)]
fn admin_instance_admins() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/admin/legal",
    tag = "admin",
    security(("fvoci_session" = [])),
    request_body = LegalPublishBody,
    responses(
        (status = 201, description = "Next version published", body = LegalDocumentOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
    )
)]
fn admin_publish_legal() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/admin/branding/assets/{asset}",
    tag = "admin",
    security(("fvoci_session" = [])),
    params(("asset" = String, Path, description = "logo or favicon")),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "PNG, APNG, WebP or JPEG, at most 512 KiB"),
    responses(
        (status = 200, description = "Settings after the upload", body = AdminInstanceSettingsOutput),
        (status = 400, description = "Empty body or invalid asset", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin", body = ProblemResponse),
        (status = 413, description = "Larger than 512 KiB", body = ProblemResponse),
        (status = 415, description = "Not application/octet-stream or not a supported image", body = ProblemResponse),
    )
)]
fn admin_upload_branding_asset() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/admin/branding/assets/{asset}",
    tag = "admin",
    security(("fvoci_session" = [])),
    params(("asset" = String, Path, description = "logo or favicon")),
    responses(
        (status = 200, description = "Settings after the removal", body = AdminInstanceSettingsOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
        (status = 404, description = "Not an instance admin or no asset", body = ProblemResponse),
    )
)]
fn admin_remove_branding_asset() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/branding/{asset}",
    tag = "admin",
    params(("asset" = String, Path, description = "logo or favicon")),
    responses(
        (status = 200, description = "Asset bytes (nosniff, CSP sandbox)", content_type = "image/*"),
        (status = 304, description = "Not modified"),
        (status = 404, description = "No asset", body = ProblemResponse),
    )
)]
fn branding_asset() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/instance",
    tag = "admin",
    responses(
        (status = 200, description = "Public instance settings", body = InstanceSettingsOutput),
        (status = 304, description = "Not modified"),
        (status = 428, description = "Consent required (signed-in user)", body = ProblemResponse),
    )
)]
fn instance_settings_public() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/legal/{kind}",
    tag = "legal",
    params(
        ("kind" = String, Path, description = "Document kind"),
        ("version" = Option<i32>, Query, description = "Version (default latest)"),
    ),
    responses(
        (status = 200, description = "Legal document", body = LegalDocumentOutput),
        (status = 400, description = "Invalid kind or version", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
    )
)]
fn legal_get() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/legal/{kind}/versions",
    tag = "legal",
    params(("kind" = String, Path, description = "Document kind")),
    responses(
        (status = 200, description = "Published versions, newest first", body = LegalVersionsResponse),
        (status = 400, description = "Invalid kind", body = ProblemResponse),
    )
)]
fn legal_versions() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/auth/consents/pending",
    tag = "legal",
    security(("fvoci_session" = [])),
    responses(
        (status = 200, description = "Required documents still to accept", body = ConsentsPendingResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
    )
)]
fn pending_consents() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/auth/consents",
    tag = "legal",
    security(("fvoci_session" = [])),
    request_body = ConsentsSubmitBody,
    responses(
        (status = 200, description = "Consents recorded", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 403, description = "Origin mismatch", body = ProblemResponse),
    )
)]
fn submit_consents() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/consents",
    tag = "legal",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Members' consents", body = WorkspaceConsentsResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
    )
)]
fn workspace_consents() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/document-tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("q" = Option<String>, Query, description = "Name substring"),
        ("limit" = Option<i32>, Query, description = "1..=100, default 50"),
    ),
    responses(
        (status = 200, description = "Workspace tag pool", body = DocumentTagPoolListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_document_tags() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/document-tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
    ),
    request_body = DocumentTagCreateBody,
    responses(
        (status = 201, description = "Created tag", body = DocumentTagOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Members and above only", body = ProblemResponse),
        (status = 409, description = "Tag name taken", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn create_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("tag_id" = String, description = "Tag id"),
    ),
    request_body = DocumentTagPatchBody,
    responses(
        (status = 200, description = "Updated tag", body = DocumentTagOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Admins only", body = ProblemResponse),
        (status = 409, description = "Tag name taken", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn update_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("tag_id" = String, description = "Tag id"),
    ),
    responses(
        (status = 200, description = "Deleted", body = OkResponse),
        (status = 403, description = "Admins only", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn delete_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Assigned tags", body = DocumentTagListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_wiki_document_tags() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = DocumentTagAssignBody,
    responses(
        (status = 200, description = "Assigned tag", body = DocumentTagOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn assign_wiki_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags/{tag_id}",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("tag_id" = String, description = "Tag id"),
    ),
    responses(
        (status = 200, description = "Unassigned", body = OkResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn unassign_wiki_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Assigned tags", body = DocumentTagListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_project_document_tags() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = DocumentTagAssignBody,
    responses(
        (status = 200, description = "Assigned tag", body = DocumentTagOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn assign_project_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags/{tag_id}",
    tag = "document-tags",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("tag_id" = String, description = "Tag id"),
    ),
    responses(
        (status = 200, description = "Unassigned", body = OkResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn unassign_project_document_tag() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/collections",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
    ),
    responses(
        (status = 200, description = "Readable collections", body = CollectionListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_collections() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/collections",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
    ),
    request_body = CollectionCreateBody,
    responses(
        (status = 201, description = "Created collection", body = CollectionOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn create_collection() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    responses(
        (status = 200, description = "Fields with options", body = CollectionFieldListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_collection_fields() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    request_body = CollectionFieldCreateBody,
    responses(
        (status = 201, description = "Created field", body = CollectionFieldOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn create_collection_field() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields/{field_id}",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
        ("field_id" = String, description = "Field id"),
    ),
    request_body = CollectionFieldPatchBody,
    responses(
        (status = 200, description = "Updated field", body = CollectionFieldOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Version mismatch or project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn update_collection_field() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/items",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    request_body = CollectionAttachBody,
    responses(
        (status = 201, description = "Item (idempotent)", body = CollectionItemOutput),
        (status = 400, description = "Invalid input or scope mismatch", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn attach_collection_item() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/items/{item_id}/values",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
        ("item_id" = String, description = "Item id"),
    ),
    request_body = CollectionValueBody,
    responses(
        (status = 200, description = "New item version", body = CollectionValueResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Edit access required", body = ProblemResponse),
        (status = 409, description = "Version mismatch or archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn put_collection_value() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/query",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    request_body = CollectionQueryBody,
    responses(
        (status = 200, description = "Query page", body = CollectionQueryResponse),
        (status = 400, description = "Invalid input or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn query_collection() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    responses(
        (status = 200, description = "Own and shared views", body = CollectionViewListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_collection_views() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
    ),
    request_body = CollectionViewBody,
    responses(
        (status = 201, description = "Created view", body = CollectionViewOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Shared views need manage", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn create_collection_view() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
        ("view_id" = String, description = "View id"),
    ),
    request_body = CollectionViewBody,
    responses(
        (status = 200, description = "Updated view", body = CollectionViewOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 403, description = "Shared views need manage", body = ProblemResponse),
        (status = 409, description = "Version mismatch or project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn update_collection_view() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("collection_id" = String, description = "Collection id"),
        ("view_id" = String, description = "View id"),
    ),
    responses(
        (status = 200, description = "Deleted", body = OkResponse),
        (status = 403, description = "Shared views need manage", body = ProblemResponse),
        (status = 409, description = "Project archived", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn delete_collection_view() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/collection-item",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Item or null", body = CollectionItemLookupResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn get_document_collection_item() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/collection-item",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Item or null", body = CollectionItemLookupResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn get_task_collection_item() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/collection",
    tag = "collections",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Project task collection", body = ProjectCollectionOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn get_project_collection() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/views",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    responses(
        (status = 200, description = "Own saved views", body = ProjectViewListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn list_project_views() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/views",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
    ),
    request_body = ProjectViewCreateBody,
    responses(
        (status = 201, description = "Created view", body = ProjectViewOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn create_project_view() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/views/{view_id}",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("view_id" = String, description = "View id"),
    ),
    request_body = ProjectViewPatchBody,
    responses(
        (status = 200, description = "Updated", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 409, description = "Config changed since expectedConfig", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn update_project_view() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/views/{view_id}",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("view_id" = String, description = "View id"),
    ),
    responses(
        (status = 200, description = "Deleted", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not readable", body = ProblemResponse),
    )
)]
fn delete_project_view() {}

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

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/webhooks",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Workspace webhooks (secrets are never listed)", body = WebhookListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
    )
)]
fn list_webhooks_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/webhooks",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = WebhookCreateBody,
    responses(
        (status = 201, description = "Created webhook; the signing secret is shown only here", body = WebhookCreatedOutput),
        (status = 400, description = "Invalid URL or events", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
        (status = 503, description = "Secret encryption keys are not configured", body = ProblemResponse),
    )
)]
fn create_webhook_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("webhook_id" = String, description = "Webhook id"),
    ),
    responses(
        (status = 200, description = "Deleted webhook and its deliveries", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
    )
)]
fn remove_webhook_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/github",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "GitHub App installation", body = GithubInstallOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
    )
)]
fn get_github_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/github",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Removed the installation link", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "No installation, or not a workspace admin", body = ProblemResponse),
    )
)]
fn remove_github_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/github/install",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "GitHub App install URL with a signed state", body = GithubInstallUrlOutput),
        (status = 400, description = "GitHub App not configured or unreachable", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or not a workspace admin", body = ProblemResponse),
    )
)]
fn install_github_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/github/issue-links",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = GithubIssueLinkBody,
    responses(
        (status = 201, description = "Linked the task to an issue", body = GithubIssueLinkOutput),
        (status = 400, description = "Invalid repo/number or already linked", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Task not found or no project edit permission", body = ProblemResponse),
    )
)]
fn link_github_issue_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    get,
    path = "/api/v1/github/callback",
    tag = "integrations",
    params(
        ("state" = Option<String>, Query, description = "Signed install state"),
        ("installation_id" = Option<String>, Query, description = "GitHub installation id"),
    ),
    responses(
        (status = 302, description = "Installed; redirects to the app"),
        (status = 400, description = "Invalid or expired state", body = ProblemResponse),
    )
)]
fn github_callback_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/github/webhook",
    tag = "integrations",
    responses(
        (status = 200, description = "Accepted", body = OkResponse),
        (status = 400, description = "Not configured or invalid payload", body = ProblemResponse),
        (status = 401, description = "Signature invalid", body = ProblemResponse),
        (status = 413, description = "Body too large", body = ProblemResponse),
    )
)]
fn github_webhook_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/ai/summarize",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = AiDocumentBody,
    responses(
        (status = 200, description = "Summary", body = AiSummarizeOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "AI disabled", body = ProblemResponse),
    )
)]
fn ai_summarize_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/ai/generate-tasks",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = AiDocumentBody,
    responses(
        (status = 200, description = "Task titles", body = AiGenerateTasksOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "AI disabled", body = ProblemResponse),
    )
)]
fn ai_generate_tasks_path() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/ai/suggest-links",
    tag = "integrations",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    request_body = AiDocumentBody,
    responses(
        (status = 200, description = "Visible document ids", body = AiSuggestLinksOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "AI disabled", body = ProblemResponse),
    )
)]
fn ai_suggest_links_path() {}
