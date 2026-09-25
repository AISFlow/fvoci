use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

pub const ACTIVITY_FIELDS: &[&str] = &[
    "title",
    "type",
    "priority",
    "statusId",
    "startDate",
    "dueDate",
    "dueAt",
    "estimate",
    "parentId",
    "milestoneId",
    "recurrence",
    "archived",
    "assigneeIds",
    "labelIds",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFilter {
    All,
    Comments,
    Changes,
}

impl ActivityFilter {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "all" => Some(Self::All),
            "comments" => Some(Self::Comments),
            "changes" => Some(Self::Changes),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Comments => "comments",
            Self::Changes => "changes",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivityListQuery {
    pub filter: ActivityFilter,
    pub limit: i32,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityCursorPayload {
    pub id: Uuid,
    pub at: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub f: String,
}

pub fn activity_scope(workspace_id: Uuid, task_id: Uuid, filter: ActivityFilter) -> String {
    serde_json::to_string(&[
        workspace_id.to_string(),
        task_id.to_string(),
        filter.as_str().to_string(),
    ])
    .expect("activity scope json")
}

pub fn encode_activity_cursor(
    id: Uuid,
    created_at: DateTime<Utc>,
    item_type: &str,
    scope: &str,
) -> String {
    let payload = ActivityCursorPayload {
        id,
        at: created_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        item_type: item_type.to_string(),
        f: scope.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("activity cursor json"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityCursorError {
    Invalid,
}

/// A validated keyset position: `at` is a real timestamp, `item_type` a feed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCursor {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    pub item_type: String,
}

pub fn decode_activity_cursor(
    raw: &str,
    scope: &str,
) -> Result<ActivityCursor, ActivityCursorError> {
    if raw.len() > 1024 {
        return Err(ActivityCursorError::Invalid);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| ActivityCursorError::Invalid)?;
    let payload: ActivityCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| ActivityCursorError::Invalid)?;
    if payload.f != scope || !matches!(payload.item_type.as_str(), "change" | "comment") {
        return Err(ActivityCursorError::Invalid);
    }
    let at = DateTime::parse_from_rfc3339(&payload.at)
        .map_err(|_| ActivityCursorError::Invalid)?
        .with_timezone(&Utc);
    Ok(ActivityCursor {
        id: payload.id,
        at,
        item_type: payload.item_type,
    })
}

pub type ActivitySnapshot = serde_json::Map<String, Value>;

pub fn normalize_estimate(value: &str) -> String {
    if !value.contains('.') {
        return value.to_string();
    }
    let trimmed = value.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn value_identity(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter_map(|item| item.get("id").cloned())
                .collect(),
        ),
        Value::Object(obj) => obj.get("id").cloned().unwrap_or(Value::Null),
        other => other.clone(),
    }
}

pub fn diff_activity(
    before: Option<&ActivitySnapshot>,
    after: &ActivitySnapshot,
) -> Option<Vec<Value>> {
    let mut changes = Vec::new();
    if let Some(before) = before {
        for field in ACTIVITY_FIELDS {
            let (Some(before_value), Some(after_value)) = (before.get(*field), after.get(*field))
            else {
                continue;
            };
            if value_identity(before_value) != value_identity(after_value) {
                changes.push(json!({
                    "field": field,
                    "from": before_value,
                    "to": after_value,
                }));
            }
        }
        if changes.is_empty() {
            return None;
        }
    }
    Some(changes)
}

pub fn patch_activity_fields(input: &crate::tasks::patch::PatchTaskMetaInput) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if input.title.is_some() {
        fields.push("title");
    }
    if input.task_type.is_some() {
        fields.push("type");
    }
    if input.priority.is_some() {
        fields.push("priority");
    }
    if input.status_id.is_some() {
        fields.push("statusId");
        fields.push("recurrence");
    }
    if input.start_date != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("startDate");
    }
    if input.due_date != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("dueDate");
    }
    if input.due_at != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("dueAt");
    }
    if input.estimate != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("estimate");
    }
    if input.parent_id != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("parentId");
    }
    if input.milestone_id != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("milestoneId");
    }
    if input.recurrence != crate::tasks::patch::FieldUpdate::Unchanged {
        fields.push("recurrence");
    }
    if input.archived.is_some() {
        fields.push("archived");
    }
    if input.assignee_ids.is_some() {
        fields.push("assigneeIds");
    }
    if input.label_ids.is_some() {
        fields.push("labelIds");
    }
    fields
}
