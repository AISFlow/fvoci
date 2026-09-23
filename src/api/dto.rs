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

/// OpenAPI request-body schema for PATCH /api/v1/auth/me.
/// Runtime parsing uses [`crate::http::json_input::parse_patch_me`].
#[derive(Debug, Clone)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[cfg_attr(feature = "api-schema", schema(rename_all = "camelCase"))]
pub struct PatchMeBody {
    pub given_name: String,
    /// Omitted preserves the current value; JSON `null` clears it.
    #[cfg_attr(feature = "api-schema", schema(nullable))]
    pub family_name: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub week_starts_on: Option<i32>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_user_output_serializes_null_family_name() {
        let output = SessionUserOutput {
            user_id: "user-1".into(),
            email: "user@example.com".into(),
            given_name: "Given".into(),
            family_name: None,
            text_scale: 16,
            session_id: "session-1".into(),
            email_verified_at: None,
            has_password: true,
            is_instance_admin: false,
            locale: "ko".into(),
            timezone: "Asia/Seoul".into(),
            week_starts_on: 1,
        };
        let body = serde_json::to_value(output).expect("serialize session user");
        let object = body.as_object().expect("session user object");
        assert!(object.contains_key("familyName"), "familyName must be present");
        assert!(object["familyName"].is_null(), "familyName must be null");
    }
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
