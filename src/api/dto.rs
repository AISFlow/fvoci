use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SetupStatusResponse {
    pub needed: bool,
    pub branding: BrandingOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BrandingOutput {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SetupBody {
    pub email: String,
    pub password: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub workspace_slug: String,
    pub workspace_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SetupResponse {
    pub user_id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LoginBody {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LoginResponse {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SessionUserOutput {
    pub user_id: String,
    pub email: String,
    pub given_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,
    pub text_scale: i16,
    pub session_id: String,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub has_password: bool,
    pub is_instance_admin: bool,
    pub locale: String,
    pub timezone: String,
    pub week_starts_on: i32,
}

/// PATCH /api/v1/auth/me body. `familyName` omitted preserves the value; null clears it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchMeBody {
    pub given_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_name: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_starts_on: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_scale: Option<i16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkspaceListResponse {
    pub items: Vec<WorkspaceListItemResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkspaceListItemResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub kind: String,
    pub document_count: i32,
    pub assigned_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkspaceMetaResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MemberResponse {
    pub user_id: String,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateWorkspaceBody {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchWorkspaceBody {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MemberRoleBody {
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProblemResponse {
    pub title: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl From<crate::auth::session::SessionUser> for SessionUserOutput {
    fn from(user: crate::auth::session::SessionUser) -> Self {
        Self {
            user_id: user.user_id,
            email: user.email,
            given_name: user.given_name,
            family_name: user.family_name,
            text_scale: user.text_scale,
            session_id: user.session_id,
            email_verified_at: user.email_verified_at,
            has_password: user.has_password,
            is_instance_admin: user.is_instance_admin,
            locale: user.locale,
            timezone: user.timezone,
            week_starts_on: user.week_starts_on,
        }
    }
}
