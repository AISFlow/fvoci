use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

pub const SESSION_COOKIE: &str = "fvoci_session";
pub const API_PREFIX: &str = "/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemCode {
    AuthenticationRequired,
    InvalidEmailOrPassword,
    InvalidInput,
    InstanceSetupAlreadyCompleted,
    PasswordInvalid,
    MagicInvalid,
    SlugTaken,
    OriginMismatch,
    AssigneeIsNotAMember,
    NotFound,
    InsufficientPermissions,
    PersonalWorkspaceImmutable,
    WorkspaceLastOwnerRequired,
    WorkspaceMemberSelfChangeForbidden,
    CannotManageRoleAboveOwn,
    CannotInviteARoleAboveYourOwn,
    InvitationNotFoundOrExpired,
    Expired,
    AlreadyAccepted,
    CannotAcceptInvitation,
    ConsentRequired,
    LimitSeats,
    LimitGuests,
    RateLimitExceeded,
    UploadIsNotInTheRequiredState,
    OnlyTheUploaderMayContinueThisUpload,
    FileExceedsUploadMaxFileSizeMb,
    PartExceedsUploadPartSizeMb,
    SubmittedPartsDoNotMatchUploadedParts,
    AttachmentFailedVirusScan,
    RangeNotSatisfiable,
    Conflict,
    ProjectArchived,
    RestoreRejected,
    CollabTimeoutRetry,
    InternalError,
}

impl ProblemCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "authentication_required",
            Self::InvalidEmailOrPassword => "invalid_email_or_password",
            Self::InvalidInput => "invalid_input",
            Self::InstanceSetupAlreadyCompleted => "instance_setup_already_completed",
            Self::PasswordInvalid => "password_invalid",
            Self::MagicInvalid => "magic_invalid",
            Self::SlugTaken => "slug_taken",
            Self::OriginMismatch => "origin_mismatch",
            Self::AssigneeIsNotAMember => "assignee_is_not_a_member",
            Self::NotFound => "not_found",
            Self::InsufficientPermissions => "insufficient_permissions",
            Self::PersonalWorkspaceImmutable => "personal_workspace_is_immutable",
            Self::WorkspaceLastOwnerRequired => "workspace_last_owner_required",
            Self::WorkspaceMemberSelfChangeForbidden => "workspace_member_self_change_forbidden",
            Self::CannotManageRoleAboveOwn => "cannot_manage_a_role_above_your_own",
            Self::CannotInviteARoleAboveYourOwn => "cannot_invite_a_role_above_your_own",
            Self::InvitationNotFoundOrExpired => "invitation_not_found_or_expired",
            Self::Expired => "expired",
            Self::AlreadyAccepted => "already_accepted",
            Self::CannotAcceptInvitation => "cannot_accept_invitation",
            Self::ConsentRequired => "consent_required",
            Self::LimitSeats => "limit.seats",
            Self::LimitGuests => "limit.guests",
            Self::RateLimitExceeded => "rate_limit_exceeded",
            Self::UploadIsNotInTheRequiredState => "upload_is_not_in_the_required_state",
            Self::OnlyTheUploaderMayContinueThisUpload => {
                "only_the_uploader_may_continue_this_upload"
            }
            Self::FileExceedsUploadMaxFileSizeMb => "file_exceeds_upload_max_file_size_mb",
            Self::PartExceedsUploadPartSizeMb => "part_exceeds_upload_part_size_mb",
            Self::SubmittedPartsDoNotMatchUploadedParts => {
                "submitted_parts_do_not_match_uploaded_parts"
            }
            Self::AttachmentFailedVirusScan => "attachment_failed_virus_scan",
            Self::RangeNotSatisfiable => "range_not_satisfiable",
            Self::Conflict => "conflict",
            Self::ProjectArchived => "project_archived",
            Self::RestoreRejected => "restore_rejected",
            Self::CollabTimeoutRetry => "collab_timeout_retry",
            Self::InternalError => "internal_error",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "authentication required",
            Self::InvalidEmailOrPassword => "invalid email or password",
            Self::InvalidInput => "invalid input",
            Self::InstanceSetupAlreadyCompleted => "instance setup already completed",
            Self::PasswordInvalid => "password_invalid",
            Self::MagicInvalid => "magic_invalid",
            Self::SlugTaken => "slug taken",
            Self::OriginMismatch => "origin mismatch",
            Self::AssigneeIsNotAMember => "assignee is not a member",
            Self::NotFound => "not found",
            Self::InsufficientPermissions => "insufficient permissions",
            Self::PersonalWorkspaceImmutable => "personal workspace is immutable",
            Self::WorkspaceLastOwnerRequired => "workspace must retain an owner",
            Self::WorkspaceMemberSelfChangeForbidden => {
                "workspace members cannot change or remove themselves here"
            }
            Self::CannotManageRoleAboveOwn => "cannot manage a workspace role above your own",
            Self::CannotInviteARoleAboveYourOwn => "cannot invite a role above your own",
            Self::InvitationNotFoundOrExpired => "invitation not found or expired",
            Self::Expired => "expired",
            Self::AlreadyAccepted => "already_accepted",
            Self::CannotAcceptInvitation => "cannot accept invitation",
            Self::ConsentRequired => "consent_required",
            Self::LimitSeats => "seat limit reached",
            Self::LimitGuests => "guest limit reached",
            Self::RateLimitExceeded => "rate limit exceeded",
            Self::UploadIsNotInTheRequiredState => "upload is not in the required state",
            Self::OnlyTheUploaderMayContinueThisUpload => {
                "only the uploader may continue this upload"
            }
            Self::FileExceedsUploadMaxFileSizeMb => "file exceeds upload max file size mb",
            Self::PartExceedsUploadPartSizeMb => "part exceeds upload part size mb",
            Self::SubmittedPartsDoNotMatchUploadedParts => {
                "submitted parts do not match uploaded parts"
            }
            Self::AttachmentFailedVirusScan => "attachment failed virus scan",
            Self::RangeNotSatisfiable => "range not satisfiable",
            Self::Conflict => "conflict",
            Self::ProjectArchived => "project archived",
            Self::RestoreRejected => "restore rejected",
            Self::CollabTimeoutRetry => "collab timeout — retry",
            Self::InternalError => "internal error",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::AuthenticationRequired
            | Self::InvalidEmailOrPassword
            | Self::CannotAcceptInvitation => StatusCode::UNAUTHORIZED,
            Self::InvalidInput
            | Self::PasswordInvalid
            | Self::MagicInvalid
            | Self::AssigneeIsNotAMember => StatusCode::BAD_REQUEST,
            Self::InstanceSetupAlreadyCompleted
            | Self::NotFound
            | Self::InvitationNotFoundOrExpired => StatusCode::NOT_FOUND,
            Self::Expired | Self::AlreadyAccepted => StatusCode::GONE,
            Self::ConsentRequired => StatusCode::PRECONDITION_REQUIRED,
            Self::LimitSeats | Self::LimitGuests => StatusCode::PAYMENT_REQUIRED,
            Self::SlugTaken
            | Self::PersonalWorkspaceImmutable
            | Self::WorkspaceLastOwnerRequired
            | Self::WorkspaceMemberSelfChangeForbidden => StatusCode::CONFLICT,
            Self::InsufficientPermissions
            | Self::CannotManageRoleAboveOwn
            | Self::CannotInviteARoleAboveYourOwn
            | Self::OnlyTheUploaderMayContinueThisUpload
            | Self::AttachmentFailedVirusScan => StatusCode::FORBIDDEN,
            Self::OriginMismatch => StatusCode::FORBIDDEN,
            Self::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            Self::FileExceedsUploadMaxFileSizeMb | Self::PartExceedsUploadPartSizeMb => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            Self::UploadIsNotInTheRequiredState => StatusCode::CONFLICT,
            Self::Conflict | Self::ProjectArchived | Self::RestoreRejected => StatusCode::CONFLICT,
            Self::CollabTimeoutRetry => StatusCode::GATEWAY_TIMEOUT,
            Self::SubmittedPartsDoNotMatchUploadedParts => StatusCode::BAD_REQUEST,
            Self::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: ProblemCode,
    pub source: Option<String>,
    pub params: Option<Value>,
    pub retry_after: Option<u32>,
}

impl AppError {
    pub fn problem(status: StatusCode, code: ProblemCode) -> Self {
        Self {
            status,
            code,
            source: None,
            params: None,
            retry_after: None,
        }
    }

    pub fn from_code(code: ProblemCode) -> Self {
        Self {
            status: code.status(),
            code,
            source: None,
            params: None,
            retry_after: None,
        }
    }

    pub fn with_source(code: ProblemCode, pointer: impl Into<String>) -> Self {
        Self {
            status: code.status(),
            code,
            source: Some(pointer.into()),
            params: None,
            retry_after: None,
        }
    }

    pub fn rate_limited(retry_after: u32) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: ProblemCode::RateLimitExceeded,
            source: None,
            params: Some(json!({ "retryAfter": retry_after })),
            retry_after: Some(retry_after),
        }
    }

    pub fn internal() -> Self {
        Self::from_code(ProblemCode::InternalError)
    }
}

#[derive(Serialize)]
struct ProblemBody {
    #[serde(rename = "type")]
    kind: &'static str,
    title: String,
    status: u16,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ProblemBody {
            kind: "about:blank",
            title: self.code.title().to_string(),
            status: self.status.as_u16(),
            code: self.code.as_str().to_string(),
            source: self.source,
            params: self.params,
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(retry_after) = self.retry_after {
            if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
                headers.insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        (self.status, headers, Json(body)).into_response()
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::JsonDataError(_) => {
                AppError::with_source(ProblemCode::InvalidInput, "/")
            }
            JsonRejection::JsonSyntaxError(_) => {
                AppError::with_source(ProblemCode::InvalidInput, "/")
            }
            JsonRejection::MissingJsonContentType(_) | JsonRejection::BytesRejection(_) => {
                AppError::from_code(ProblemCode::InvalidInput)
            }
            _ => AppError::from_code(ProblemCode::InvalidInput),
        }
    }
}

impl From<QueryRejection> for AppError {
    fn from(_rejection: QueryRejection) -> Self {
        AppError::from_code(ProblemCode::InvalidInput)
    }
}
