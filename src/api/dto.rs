use chrono::{DateTime, NaiveDate, Utc};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

fn deserialize_present_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn deserialize_double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

fn deserialize_optional_non_null_i32<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i32>, D::Error> {
    match Option::<i32>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::invalid_type(
            serde::de::Unexpected::Unit,
            &"integer",
        )),
    }
}

fn deserialize_optional_non_null_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    match Option::<String>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::invalid_type(
            serde::de::Unexpected::Unit,
            &"string",
        )),
    }
}

fn deserialize_optional_non_null_uuid<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Uuid>, D::Error> {
    match Option::<Uuid>::deserialize(deserializer)? {
        Some(value) => Ok(Some(value)),
        None => Err(serde::de::Error::invalid_type(
            serde::de::Unexpected::Unit,
            &"uuid",
        )),
    }
}

fn strict_date<E: serde::de::Error>(value: String) -> Result<NaiveDate, E> {
    crate::tasks::parse_iso_date(&value).ok_or_else(|| {
        serde::de::Error::invalid_value(serde::de::Unexpected::Str(&value), &"YYYY-MM-DD date")
    })
}

fn deserialize_required_date<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<NaiveDate, D::Error> {
    strict_date(String::deserialize(deserializer)?)
}

fn deserialize_optional_non_null_date<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<NaiveDate>, D::Error> {
    match Option::<String>::deserialize(deserializer)? {
        Some(value) => strict_date(value).map(Some),
        None => Err(serde::de::Error::invalid_type(
            serde::de::Unexpected::Unit,
            &"date",
        )),
    }
}

fn deserialize_nullable_date<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<NaiveDate>, D::Error> {
    Option::<String>::deserialize(deserializer)?
        .map(strict_date)
        .transpose()
}

fn deserialize_double_option_date<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<NaiveDate>>, D::Error> {
    deserialize_nullable_date(deserializer).map(Some)
}

fn deserialize_optional_recurrence<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let Some(obj) = value.as_object() else {
        return Err(serde::de::Error::custom("invalid recurrence preset"));
    };
    if obj.len() != 1 {
        return Err(serde::de::Error::custom("invalid recurrence preset"));
    }
    for key in obj.keys() {
        if key != "kind" {
            return Err(serde::de::Error::custom("invalid recurrence preset"));
        }
    }
    let Some(kind) = obj.get("kind").and_then(Value::as_str) else {
        return Err(serde::de::Error::custom("invalid recurrence preset"));
    };
    if matches!(kind, "daily" | "weekly" | "monthly") {
        Ok(Some(json!({ "kind": kind })))
    } else {
        Err(serde::de::Error::custom("invalid recurrence preset"))
    }
}

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
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub family_name: Option<String>,
    pub text_scale: i16,
    pub session_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
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
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub locale: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub timezone: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub week_starts_on: Option<i32>,
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateHolidayBody {
    #[serde(deserialize_with = "deserialize_required_date")]
    pub date: NaiveDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct HolidaysListResponse {
    pub can_edit: bool,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct IcsTokenResponse {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationItemOutput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub event_id: Uuid,
    pub verb: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub actor_user_id: Option<Uuid>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub actor_given_name: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub actor_family_name: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub target_type: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub target_id: Option<Uuid>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub display_id: Option<String>,
    pub payload: Value,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub read_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationListResponse {
    pub items: Vec<NotificationItemOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationUnreadCountResponse {
    pub count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationPatchBody {
    #[serde(default)]
    pub read: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationReadAllResponse {
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NotificationPrefsBody {
    pub in_app: bool,
    pub mail_immediate: bool,
    pub mail_digest: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ApiTokenCreateBody {
    pub name: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub unlimited: Option<bool>,
    #[serde(default)]
    pub service: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MeApiTokenCreateBody {
    pub workspace_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ApiTokenOutput {
    pub id: String,
    pub workspace_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub user_id: Option<String>,
    pub name: String,
    pub scopes: Vec<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ApiTokenCreatedOutput {
    pub id: String,
    pub workspace_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub user_id: Option<String>,
    pub name: String,
    pub scopes: Vec<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ApiTokenListResponse {
    pub items: Vec<ApiTokenOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MemberResponse {
    pub user_id: String,
    pub email: String,
    pub given_name: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
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
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = false))]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DeleteWorkspaceBody {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = false))]
    pub confirm_slug: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MemberRoleBody {
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MembersResponse {
    pub items: Vec<MemberResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationCreateBody {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationCreateResponse {
    pub accept_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationLegalDocument {
    pub kind: String,
    pub version: i32,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationPublicResponse {
    pub workspace_name: String,
    pub email_masked: String,
    pub role: String,
    pub required_legal: Vec<InvitationLegalDocument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationConsentItem {
    pub kind: String,
    pub version: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct InvitationAcceptBody {
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    pub email: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    pub password: Option<String>,
    #[serde(default)]
    pub consents: Option<Vec<InvitationConsentItem>>,
}

/// Source `parentId` is `uuid.nullable()`: present and null is allowed, omitted is not.
/// `#[serde(default)]` plus a third Missing variant is required; a wrapper around
/// `Option` would treat omitted fields as null because serde's missing-field path
/// visits `none`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RequiredNullable<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for RequiredNullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            None => Self::Null,
            Some(value) => Self::Value(value),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateDocumentBody {
    #[cfg_attr(feature = "api-schema", schema(value_type = Option<Uuid>, required = true, nullable = true))]
    #[serde(default)]
    pub parent_id: RequiredNullable<Uuid>,
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    #[cfg_attr(feature = "api-schema", schema(nullable = true))]
    pub icon: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchDocumentBody {
    #[serde(default, deserialize_with = "deserialize_present_string")]
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    #[cfg_attr(feature = "api-schema", schema(nullable = true))]
    pub icon: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentMetaResponse {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub number: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub icon: Option<String>,
    pub path: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub parent_id: Option<String>,
    pub sort_key: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
    pub status: String,
    pub schema_version: i32,
    pub version: i32,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "api-schema", schema(required = false, nullable = false))]
    pub display_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TreeResponse {
    pub items: Vec<TreeNodeResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TreeNodeResponse {
    pub id: String,
    pub workspace_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub parent_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub icon: Option<String>,
    pub path: String,
    pub sort_key: String,
    pub number: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AncestorsResponse {
    pub items: Vec<AncestorResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AncestorResponse {
    pub id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub icon: Option<String>,
    pub path: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
    pub number: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BodyResponse {
    pub content_json: Value,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionCreateResponse {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionMetaResponse {
    pub id: String,
    pub target_kind: String,
    pub target_id: String,
    pub reason: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionListResponse {
    pub items: Vec<RevisionMetaResponse>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionDetailResponse {
    pub id: String,
    pub target_kind: String,
    pub target_id: String,
    pub reason: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub created_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub content_json: Value,
    pub y_snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionRestoreBody {
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub correlation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct RevisionRestoreResponse {
    pub restored: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MoveDocumentBody {
    pub new_parent_id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SortDocumentBody {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub after_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TrashListResponse {
    pub items: Vec<TrashItemResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TrashItemResponse {
    pub id: String,
    pub title: String,
    pub deleted_at: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateAttachmentUploadBody {
    pub name: String,
    pub size_bytes: i64,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    pub declared_mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentPartUrlResponse {
    pub part_number: i32,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateAttachmentUploadResponse {
    pub attachment_id: String,
    pub part_size_bytes: i64,
    pub parts: Vec<AttachmentPartUrlResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentUploadedPartResponse {
    pub part_number: i32,
    pub etag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ResumeAttachmentUploadResponse {
    pub attachment_id: String,
    pub part_size_bytes: i64,
    pub uploaded_parts: Vec<AttachmentUploadedPartResponse>,
    pub parts: Vec<AttachmentPartUrlResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentCompletePartBody {
    pub part_number: i32,
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CompleteAttachmentUploadBody {
    pub parts: Vec<AttachmentCompletePartBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentPreviewResponse {
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentOutput {
    pub id: String,
    pub name: String,
    pub mime: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub size_bytes: Option<i64>,
    pub image: bool,
    pub scan_status: String,
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub completed_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub preview: Option<AttachmentPreviewResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PutAttachmentPartResponse {
    pub etag: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AttachmentDownloadQuery {
    #[serde(default)]
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateProjectBody {
    pub key: String,
    pub name: String,
    pub visibility: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub lead_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchProjectBody {
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_string")]
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub visibility: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub icon: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub lead_user_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectOutput {
    pub id: String,
    pub workspace_id: String,
    pub key: String,
    pub name: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub description: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub icon: Option<String>,
    pub visibility: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub root_document_id: Option<String>,
    pub status: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectListItemOutput {
    pub id: String,
    pub workspace_id: String,
    pub key: String,
    pub name: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub description: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub icon: Option<String>,
    pub visibility: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub root_document_id: Option<String>,
    pub status: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub task_count: i64,
    pub open_task_count: i64,
    pub can_edit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectListResponse {
    pub items: Vec<ProjectListItemOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectMembersResponse {
    pub items: Vec<MemberResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct AddProjectMemberBody {
    pub user_id: Uuid,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateGroupBody {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct GroupOutput {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct GroupListResponse {
    pub items: Vec<GroupOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct GroupMemberBody {
    pub user_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct GroupMemberOutput {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct GroupMemberListResponse {
    pub items: Vec<GroupMemberOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectGroupGrantBody {
    pub group_id: Uuid,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectGroupRevokeBody {
    pub group_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectGroupGrantOutput {
    pub group_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectGroupGrantListResponse {
    pub items: Vec<ProjectGroupGrantOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkflowStatusOutput {
    pub id: String,
    pub workflow_id: String,
    pub name: String,
    pub category: String,
    pub sort_key: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub wip_limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkflowOutput {
    pub id: String,
    pub project_id: String,
    pub statuses: Vec<WorkflowStatusOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateTaskBody {
    pub title: String,
    #[serde(default = "default_task_type", rename = "type")]
    pub task_type: String,
    #[serde(default = "default_task_priority")]
    pub priority: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub status_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_date")]
    pub start_date: Option<NaiveDate>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_date")]
    pub due_date: Option<NaiveDate>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub parent_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub milestone_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_recurrence")]
    pub recurrence: Option<serde_json::Value>,
}

fn default_task_type() -> String {
    "task".to_string()
}

fn default_task_priority() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ExpectedDatesBody {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    #[serde(deserialize_with = "deserialize_nullable_date")]
    pub start_date: Option<NaiveDate>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    #[serde(deserialize_with = "deserialize_nullable_date")]
    pub due_date: Option<NaiveDate>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchTaskBody {
    pub expected_dates: Option<ExpectedDatesBody>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    #[serde(rename = "type")]
    pub task_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub priority: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub status_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_double_option_date")]
    pub start_date: Option<Option<NaiveDate>>,
    #[serde(default, deserialize_with = "deserialize_double_option_date")]
    pub due_date: Option<Option<NaiveDate>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub due_at: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub estimate: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub milestone_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub recurrence: Option<Option<Value>>,
    pub archived: Option<bool>,
    pub assignee_ids: Option<Vec<Uuid>>,
    pub label_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MoveTaskBody {
    pub status_id: Uuid,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub expected_status_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub before_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_uuid")]
    pub after_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskMetaOutput {
    pub id: String,
    pub workspace_id: String,
    pub project_id: String,
    pub number: i32,
    pub title: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub priority: String,
    pub status_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub start_date: Option<NaiveDate>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub due_date: Option<NaiveDate>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub due_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub estimate: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub parent_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub milestone_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub recurrence: Option<serde_json::Value>,
    pub sort_key: String,
    pub schema_version: i32,
    pub version: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub archived_at: Option<DateTime<Utc>>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskStatusCountOutput {
    pub status_id: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskListItemOutput {
    #[serde(flatten)]
    pub meta: TaskMetaOutput,
    pub assignee_ids: Vec<String>,
    pub label_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskDependencyOutput {
    pub blocker_id: String,
    pub blocked_id: String,
    #[serde(rename = "type")]
    pub dependency_type: String,
    pub lag_days: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskListResponse {
    pub items: Vec<TaskListItemOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub next_cursor: Option<String>,
    pub status_counts: Vec<TaskStatusCountOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskParentOutput {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub number: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskChildOutput {
    pub id: String,
    pub number: i32,
    pub title: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub status_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskChildProgressOutput {
    pub done: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskOutput {
    #[serde(flatten)]
    pub meta: TaskMetaOutput,
    pub content_json: serde_json::Value,
    pub can_edit: bool,
    pub assignee_ids: Vec<String>,
    pub label_ids: Vec<String>,
    pub dependencies: Vec<TaskDependencyOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub child_progress: Option<TaskChildProgressOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub parent: Option<TaskParentOutput>,
    pub children: Vec<TaskChildOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LabelOutput {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LabelListResponse {
    pub items: Vec<LabelOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateLabelBody {
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchLabelBody {
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MilestoneOutput {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub due_date: Option<NaiveDate>,
    pub sort_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MilestoneListResponse {
    pub items: Vec<MilestoneOutput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateMilestoneBody {
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_double_option_date")]
    pub due_date: Option<Option<NaiveDate>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchMilestoneBody {
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option_date")]
    pub due_date: Option<Option<NaiveDate>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateTaskDependencyBody {
    pub blocked_id: Uuid,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    #[serde(rename = "type")]
    pub dependency_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null_i32")]
    #[cfg_attr(feature = "api-schema", schema(nullable = false))]
    pub lag_days: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskDependencyListResponse {
    pub items: Vec<TaskDependencyOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LookupItemOutput {
    pub kind: String,
    pub id: String,
    pub display_id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LookupListResponse {
    pub items: Vec<LookupItemOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CreateCommentBody {
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentioned_user_ids: Option<Vec<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentioned_group_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchCommentBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CommentReactionBody {
    pub emoji: String,
    pub on: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CommentReactionSummary {
    pub count: usize,
    pub reacted_by_me: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CommentOutput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub created_by: Uuid,
    pub body: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub reactions: std::collections::HashMap<String, CommentReactionSummary>,
    pub other_reaction_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CommentListResponse {
    pub items: Vec<CommentOutput>,
    pub next_cursor: Option<String>,
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
        assert!(
            object.contains_key("familyName"),
            "familyName must be present"
        );
        assert!(object["familyName"].is_null(), "familyName must be null");
    }

    #[test]
    fn create_attachment_upload_rejects_unknown_fields_and_null_mime() {
        let omitted: CreateAttachmentUploadBody =
            serde_json::from_str(r#"{"name":"a.bin","sizeBytes":1}"#).unwrap();
        assert!(omitted.declared_mime.is_none());
        let with_mime: CreateAttachmentUploadBody =
            serde_json::from_str(r#"{"name":"a.bin","sizeBytes":1,"declaredMime":"text/plain"}"#)
                .unwrap();
        assert_eq!(with_mime.declared_mime.as_deref(), Some("text/plain"));
        assert!(serde_json::from_str::<CreateAttachmentUploadBody>(
            r#"{"name":"a.bin","sizeBytes":1,"declaredMime":null}"#
        )
        .is_err());
        assert!(serde_json::from_str::<CreateAttachmentUploadBody>(
            r#"{"name":"a.bin","sizeBytes":1,"extra":true}"#
        )
        .is_err());
    }

    #[test]
    fn complete_attachment_upload_rejects_unknown_fields() {
        let ok: CompleteAttachmentUploadBody =
            serde_json::from_str(r#"{"parts":[{"partNumber":1,"etag":"abc"}]}"#).unwrap();
        assert_eq!(ok.parts.len(), 1);
        assert!(serde_json::from_str::<CompleteAttachmentUploadBody>(
            r#"{"parts":[{"partNumber":1,"etag":"abc"}],"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<CompleteAttachmentUploadBody>(
            r#"{"parts":[{"partNumber":1,"etag":"abc","extra":true}]}"#
        )
        .is_err());
    }

    #[test]
    fn attachment_output_keeps_null_size_completed_and_preview() {
        let output = AttachmentOutput {
            id: "att-1".into(),
            name: "파일.png".into(),
            mime: "application/octet-stream".into(),
            size_bytes: None,
            image: false,
            scan_status: "skipped".into(),
            created_at: DateTime::parse_from_rfc3339("2026-09-24T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            completed_at: None,
            preview: None,
        };
        let body = serde_json::to_value(output).expect("serialize attachment");
        assert!(body["sizeBytes"].is_null());
        assert!(body["completedAt"].is_null());
        assert!(body["preview"].is_null());
        assert_eq!(body["name"], "파일.png");
        assert!(body["createdAt"].as_str().unwrap().contains("2026-09-24"));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SearchSnippetPiece {
    pub text: String,
    pub r#match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SearchItemOutput {
    pub r#type: String,
    pub id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub display_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub extract_status: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub chunk_no: Option<i64>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub snippet: Option<Vec<SearchSnippetPiece>>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub project_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub document_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub task_id: Option<String>,
    pub score: f64,
    pub updated_at: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct SearchListResponse {
    pub items: Vec<SearchItemOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub next_cursor: Option<String>,
}
