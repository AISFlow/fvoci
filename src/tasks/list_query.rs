use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const QUERY_JSON_MAX: usize = 16_000;
const TITLE_MAX: usize = 1_000;
const CURSOR_MAX: usize = 1_024;
const TASK_TYPES: &[&str] = &["task", "bug", "story", "epic", "subtask"];
const PRIORITIES: &[&str] = &["none", "low", "medium", "high", "urgent"];
const UNSUPPORTED_FILTER_KEYS: &[&str] = &["milestoneId", "custom", "dueBefore"];

#[derive(Debug, Clone)]
pub struct ParsedTaskListQuery {
    pub view: ViewQuery,
    pub archived: bool,
    pub limit: i32,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub cursor: Option<TaskListCursor>,
    pub as_of: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct ViewQuery {
    pub filters: ViewFilters,
    pub sort: Vec<ViewSort>,
}

#[derive(Debug, Clone, Default)]
pub struct ViewFilters {
    pub task_type: Option<String>,
    pub status_id: Option<Uuid>,
    pub priority: Option<String>,
    pub open_only: bool,
    pub title: Option<String>,
    pub assignee_id: Option<AssigneeFilter>,
    pub label_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssigneeFilter {
    Me,
    User(Uuid),
}

#[derive(Debug, Clone)]
pub struct ViewSort {
    pub field: SortField,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Priority,
    Due,
    Updated,
    Created,
    Rank,
    Title,
    Status,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct TaskListCursor {
    pub id: Uuid,
    pub key: String,
    pub f: String,
    pub as_of: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListQueryError {
    InvalidInput,
    InvalidCursor,
}

pub fn parse_task_list_query(
    query_json: Option<&str>,
    archived: Option<&str>,
    cursor: Option<&str>,
    limit: Option<i32>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<ParsedTaskListQuery, TaskListQueryError> {
    let archived = match archived {
        Some("true") => true,
        Some("false") | None => false,
        Some(_) => return Err(TaskListQueryError::InvalidInput),
    };
    let from = parse_optional_date(from)?;
    let to = parse_optional_date(to)?;
    if (from.is_some()) != (to.is_some()) {
        return Err(TaskListQueryError::InvalidInput);
    }
    if let (Some(from), Some(to)) = (from, to) {
        if from > to {
            return Err(TaskListQueryError::InvalidInput);
        }
    }
    if from.is_some() && cursor.is_some() {
        return Err(TaskListQueryError::InvalidInput);
    }
    let limit = limit.unwrap_or(50);
    if !(1..=500).contains(&limit) {
        return Err(TaskListQueryError::InvalidInput);
    }
    if from.is_none() && limit > 100 {
        return Err(TaskListQueryError::InvalidInput);
    }
    let raw_query = query_json.unwrap_or("{}");
    if raw_query.len() > QUERY_JSON_MAX {
        return Err(TaskListQueryError::InvalidInput);
    }
    let view = parse_view_query(raw_query)?;
    let (cursor, as_of) = match cursor {
        Some(raw) => {
            if raw.len() > CURSOR_MAX {
                return Err(TaskListQueryError::InvalidCursor);
            }
            let decoded = decode_cursor(raw)?;
            let as_of = decoded.as_of;
            (Some(decoded), as_of)
        }
        None => (None, Utc::now()),
    };
    Ok(ParsedTaskListQuery {
        view,
        archived,
        limit,
        from,
        to,
        cursor,
        as_of,
    })
}

fn parse_optional_date(raw: Option<&str>) -> Result<Option<NaiveDate>, TaskListQueryError> {
    match raw {
        None => Ok(None),
        Some(value) => crate::tasks::parse_iso_date(value)
            .map(Some)
            .ok_or(TaskListQueryError::InvalidInput),
    }
}

fn parse_view_query(raw: &str) -> Result<ViewQuery, TaskListQueryError> {
    let value: Value = serde_json::from_str(raw).map_err(|_| TaskListQueryError::InvalidInput)?;
    let root = value.as_object().ok_or(TaskListQueryError::InvalidInput)?;
    reject_unknown_keys(root, &["filters", "sort"])?;

    let filters = if let Some(filters_value) = root.get("filters") {
        parse_view_filters(filters_value)?
    } else {
        ViewFilters::default()
    };
    let sort = if let Some(sort_value) = root.get("sort") {
        parse_view_sort(sort_value)?
    } else {
        Vec::new()
    };
    Ok(ViewQuery { filters, sort })
}

fn parse_view_filters(value: &Value) -> Result<ViewFilters, TaskListQueryError> {
    let object = value.as_object().ok_or(TaskListQueryError::InvalidInput)?;
    reject_unknown_keys(
        object,
        &[
            "type",
            "statusId",
            "priority",
            "openOnly",
            "title",
            "assigneeId",
            "labelId",
        ],
    )?;
    for key in UNSUPPORTED_FILTER_KEYS {
        if object.contains_key(*key) {
            return Err(TaskListQueryError::InvalidInput);
        }
    }

    let task_type = match object.get("type") {
        None => None,
        Some(value) => Some(parse_task_type(value)?),
    };
    let status_id = match object.get("statusId") {
        None => None,
        Some(value) => Some(parse_uuid(value, "statusId")?),
    };
    let priority = match object.get("priority") {
        None => None,
        Some(value) => Some(parse_priority(value)?),
    };
    let open_only = match object.get("openOnly") {
        None => false,
        Some(value) => value.as_bool().ok_or(TaskListQueryError::InvalidInput)?,
    };
    let title = match object.get("title") {
        None => None,
        Some(value) => Some(parse_title(value)?),
    };
    let assignee_id = match object.get("assigneeId") {
        None => None,
        Some(value) => Some(parse_assignee_id(value)?),
    };
    let label_id = match object.get("labelId") {
        None => None,
        Some(value) => Some(parse_uuid(value, "labelId")?),
    };

    Ok(ViewFilters {
        task_type,
        status_id,
        priority,
        open_only,
        title,
        assignee_id,
        label_id,
    })
}

fn parse_view_sort(value: &Value) -> Result<Vec<ViewSort>, TaskListQueryError> {
    let array = value.as_array().ok_or(TaskListQueryError::InvalidInput)?;
    if array.len() > 3 {
        return Err(TaskListQueryError::InvalidInput);
    }
    let mut sort = Vec::with_capacity(array.len());
    for entry in array {
        let object = entry.as_object().ok_or(TaskListQueryError::InvalidInput)?;
        reject_unknown_keys(object, &["field", "direction"])?;
        let field_value = object
            .get("field")
            .ok_or(TaskListQueryError::InvalidInput)?;
        let direction_value = object
            .get("direction")
            .ok_or(TaskListQueryError::InvalidInput)?;
        let field = parse_sort_field(field_value)?;
        let direction = parse_sort_direction(direction_value)?;
        sort.push(ViewSort { field, direction });
    }
    Ok(sort)
}

fn parse_task_type(value: &Value) -> Result<String, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    if TASK_TYPES.contains(&raw) {
        Ok(raw.to_string())
    } else {
        Err(TaskListQueryError::InvalidInput)
    }
}

fn parse_priority(value: &Value) -> Result<String, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    if PRIORITIES.contains(&raw) {
        Ok(raw.to_string())
    } else {
        Err(TaskListQueryError::InvalidInput)
    }
}

fn parse_title(value: &Value) -> Result<String, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    if raw.contains('\0') || raw.chars().count() > TITLE_MAX || raw.trim().is_empty() {
        return Err(TaskListQueryError::InvalidInput);
    }
    Ok(raw.to_string())
}

fn parse_uuid(value: &Value, _field: &str) -> Result<Uuid, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    Uuid::parse_str(raw).map_err(|_| TaskListQueryError::InvalidInput)
}

fn parse_assignee_id(value: &Value) -> Result<AssigneeFilter, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    if raw == "me" {
        Ok(AssigneeFilter::Me)
    } else {
        Uuid::parse_str(raw)
            .map(AssigneeFilter::User)
            .map_err(|_| TaskListQueryError::InvalidInput)
    }
}

fn parse_sort_field(value: &Value) -> Result<SortField, TaskListQueryError> {
    let raw = value.as_str().ok_or(TaskListQueryError::InvalidInput)?;
    if Uuid::parse_str(raw).is_ok() {
        return Err(TaskListQueryError::InvalidInput);
    }
    match raw {
        "priority" => Ok(SortField::Priority),
        "due" => Ok(SortField::Due),
        "updated" => Ok(SortField::Updated),
        "created" => Ok(SortField::Created),
        "rank" => Ok(SortField::Rank),
        "title" => Ok(SortField::Title),
        "status" => Ok(SortField::Status),
        "number" => Ok(SortField::Number),
        _ => Err(TaskListQueryError::InvalidInput),
    }
}

fn parse_sort_direction(value: &Value) -> Result<SortDirection, TaskListQueryError> {
    match value.as_str() {
        Some("asc") => Ok(SortDirection::Asc),
        Some("desc") => Ok(SortDirection::Desc),
        _ => Err(TaskListQueryError::InvalidInput),
    }
}

fn reject_unknown_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), TaskListQueryError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(TaskListQueryError::InvalidInput);
        }
    }
    Ok(())
}

pub fn filter_fingerprint(
    workspace_id: Uuid,
    project_id: Uuid,
    query: &ParsedTaskListQuery,
) -> String {
    let assignee_id = match &query.view.filters.assignee_id {
        Some(AssigneeFilter::Me) => Some("me".to_string()),
        Some(AssigneeFilter::User(id)) => Some(id.to_string()),
        None => None,
    };
    let payload = serde_json::json!({
        "workspaceId": workspace_id.to_string(),
        "projectId": project_id.to_string(),
        "query": {
            "filters": {
                "type": query.view.filters.task_type,
                "statusId": query.view.filters.status_id.map(|id| id.to_string()),
                "priority": query.view.filters.priority,
                "openOnly": query.view.filters.open_only,
                "title": query.view.filters.title,
                "assigneeId": assignee_id,
                "labelId": query.view.filters.label_id.map(|id| id.to_string()),
            },
            "sort": query.view.sort.iter().map(|sort| {
                serde_json::json!({
                    "field": sort_field_name(sort.field),
                    "direction": if sort.direction == SortDirection::Asc { "asc" } else { "desc" },
                })
            }).collect::<Vec<_>>(),
        },
        "archived": query.archived,
        "from": query.from,
        "to": query.to,
    });
    sha256_hex(payload.to_string())
}

pub fn encode_cursor(cursor: &TaskListCursor) -> String {
    use base64::Engine;
    let payload = serde_json::json!({
        "id": cursor.id.to_string(),
        "key": cursor.key,
        "f": cursor.f,
        "asOf": cursor.as_of.to_rfc3339(),
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

fn decode_cursor(raw: &str) -> Result<TaskListCursor, TaskListQueryError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| TaskListQueryError::InvalidCursor)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| TaskListQueryError::InvalidCursor)?;
    let object = value.as_object().ok_or(TaskListQueryError::InvalidCursor)?;
    reject_unknown_keys(object, &["id", "key", "f", "asOf"])?;
    let id = parse_uuid(
        object.get("id").ok_or(TaskListQueryError::InvalidCursor)?,
        "id",
    )?;
    let key = object
        .get("key")
        .and_then(Value::as_str)
        .ok_or(TaskListQueryError::InvalidCursor)?;
    let f = object
        .get("f")
        .and_then(Value::as_str)
        .ok_or(TaskListQueryError::InvalidCursor)?;
    let as_of = object
        .get("asOf")
        .and_then(Value::as_str)
        .ok_or(TaskListQueryError::InvalidCursor)?;
    if !is_lower_hex_64(key) || !is_lower_hex_64(f) {
        return Err(TaskListQueryError::InvalidCursor);
    }
    let as_of = DateTime::parse_from_rfc3339(as_of)
        .map_err(|_| TaskListQueryError::InvalidCursor)?
        .with_timezone(&Utc);
    Ok(TaskListCursor {
        id,
        key: key.to_string(),
        f: f.to_string(),
        as_of,
    })
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

pub fn sort_field_name(field: SortField) -> &'static str {
    match field {
        SortField::Priority => "priority",
        SortField::Due => "due",
        SortField::Updated => "updated",
        SortField::Created => "created",
        SortField::Rank => "rank",
        SortField::Title => "title",
        SortField::Status => "status",
        SortField::Number => "number",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cursor_key_for_row(
    sort: &[ViewSort],
    id: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    number: i32,
    title: &str,
    sort_key: &str,
    priority: &str,
    status_sort_key: &str,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> String {
    let sort = effective_sort_entries(sort);
    let mut parts = Vec::with_capacity(sort.len() + 1);
    for entry in sort {
        parts.push(format!(
            "{}:{}",
            sort_field_name(entry.field),
            sort_value_token(
                entry.field,
                created_at,
                updated_at,
                number,
                title,
                sort_key,
                priority,
                status_sort_key,
                due_date,
                due_at,
            )
        ));
    }
    parts.push(format!("id:{id}"));
    sha256_hex(parts.join("\0"))
}

pub fn default_task_sort() -> Vec<ViewSort> {
    vec![ViewSort {
        field: SortField::Rank,
        direction: SortDirection::Asc,
    }]
}

pub fn effective_sort_entries(sort: &[ViewSort]) -> Vec<ViewSort> {
    if sort.is_empty() {
        default_task_sort()
    } else {
        sort.to_vec()
    }
}

pub fn effective_due_date(
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> Option<NaiveDate> {
    due_date.or_else(|| due_at.map(|value| value.date_naive()))
}

#[allow(clippy::too_many_arguments)]
pub fn sort_value_token(
    field: SortField,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    number: i32,
    title: &str,
    sort_key: &str,
    priority: &str,
    status_sort_key: &str,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> String {
    match field {
        SortField::Created => created_at.to_rfc3339(),
        SortField::Updated => updated_at.to_rfc3339(),
        SortField::Number => number.to_string(),
        SortField::Title => title.to_string(),
        SortField::Rank => sort_key.to_string(),
        SortField::Priority => priority_rank(priority).to_string(),
        SortField::Status => status_sort_key.to_string(),
        SortField::Due => effective_due_date(due_date, due_at)
            .map(|date| date.to_string())
            .unwrap_or_else(|| "null".to_string()),
    }
}

pub fn priority_rank(priority: &str) -> i32 {
    match priority {
        "none" => 0,
        "low" => 1,
        "medium" => 2,
        "high" => 3,
        "urgent" => 4,
        _ => 0,
    }
}

fn sha256_hex(value: String) -> String {
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_filter_keys() {
        let err = parse_task_list_query(
            Some(r#"{"filters":{"milestoneId":"00000000-0000-0000-0000-000000000000"}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(err, TaskListQueryError::InvalidInput);
    }

    #[test]
    fn accepts_assignee_and_label_filters() {
        let parsed = parse_task_list_query(
            Some(
                r#"{"filters":{"assigneeId":"me","labelId":"550e8400-e29b-41d4-a716-446655440000"}}"#,
            ),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("valid query");
        assert_eq!(parsed.view.filters.assignee_id, Some(AssigneeFilter::Me));
        assert_eq!(
            parsed.view.filters.label_id,
            Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap())
        );
    }

    #[test]
    fn rejects_invalid_assignee_filter() {
        let err = parse_task_list_query(
            Some(r#"{"filters":{"assigneeId":"everyone"}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(err, TaskListQueryError::InvalidInput);
    }

    #[test]
    fn fingerprint_includes_assignee_and_label() {
        let workspace = Uuid::nil();
        let project = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let base = parse_task_list_query(Some("{}"), None, None, None, None, None).unwrap();
        let filtered = parse_task_list_query(
            Some(r#"{"filters":{"assigneeId":"me"}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_ne!(
            filter_fingerprint(workspace, project, &base),
            filter_fingerprint(workspace, project, &filtered)
        );
    }

    #[test]
    fn rejects_invalid_task_type_instead_of_dropping() {
        let err = parse_task_list_query(
            Some(r#"{"filters":{"type":"milestone"}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(err, TaskListQueryError::InvalidInput);
    }

    #[test]
    fn accepts_camel_case_status_and_open_only() {
        let parsed = parse_task_list_query(
            Some(r#"{"filters":{"statusId":"550e8400-e29b-41d4-a716-446655440000","openOnly":true}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("valid query");
        assert_eq!(
            parsed.view.filters.status_id,
            Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap())
        );
        assert!(parsed.view.filters.open_only);
    }

    #[test]
    fn rejects_snake_case_status_id() {
        let err = parse_task_list_query(
            Some(r#"{"filters":{"status_id":"550e8400-e29b-41d4-a716-446655440000"}}"#),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(err, TaskListQueryError::InvalidInput);
    }

    #[test]
    fn rejects_oversized_query_json() {
        let huge = format!("{{\"filters\":{{\"title\":\"{}\"}}}}", "a".repeat(16_001));
        let err = parse_task_list_query(Some(&huge), None, None, None, None, None).unwrap_err();
        assert_eq!(err, TaskListQueryError::InvalidInput);
    }

    #[test]
    fn preserves_cursor_as_of_on_decode() {
        let as_of = Utc::now();
        let encoded = encode_cursor(&TaskListCursor {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            key: "a".repeat(64),
            f: "b".repeat(64),
            as_of,
        });
        let parsed = parse_task_list_query(None, None, Some(&encoded), None, None, None)
            .expect("valid cursor");
        assert_eq!(parsed.as_of.timestamp(), as_of.timestamp());
    }
}
