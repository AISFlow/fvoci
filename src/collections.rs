//! Collections, collection views and project saved views: input contracts.
//!
//! Source `packages/contracts/src/collections.ts`, `view-query.ts` and the
//! `views` inputs in `tasks.ts`. Bodies are strict objects: unknown keys,
//! wrong JSON types and out-of-range values are `invalid_input`.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::tasks::list_query::{parse_view_query_value, view_query_to_json, ViewQuery};
use crate::tasks::{parse_iso_date, parse_iso_datetime};

pub const NAME_MAX: usize = 100;
pub const FIELD_KEY_MAX: usize = 50;
pub const FIELD_DESCRIPTION_MAX: usize = 2000;
pub const FIELDS_PER_COLLECTION_MAX: usize = 50;
pub const CREATE_OPTIONS_MAX: usize = 50;
pub const PATCH_OPTIONS_MAX: usize = 200;
pub const VALUE_TEXT_MAX: usize = 10_000;
pub const VALUE_OPTIONS_MAX: usize = 200;
pub const VALUE_USERS_MAX: usize = 100;
pub const QUERY_LIMIT_MAX: i64 = 100;
pub const QUERY_LIMIT_DEFAULT: i64 = 50;
pub const GROUP_MAX: usize = 200;
pub const CURSOR_MAX: usize = 1024;
pub const TIME_ZONE_MAX: usize = 100;
pub const WINDOW_DAYS_MAX: i64 = 366;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidInput;

type Parsed<T> = Result<T, InvalidInput>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Paragraph,
    Number,
    Date,
    Datetime,
    Checkbox,
    Select,
    MultiSelect,
    Checkboxes,
    User,
    UserMulti,
    Labels,
}

impl FieldType {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "text" => Self::Text,
            "paragraph" => Self::Paragraph,
            "number" => Self::Number,
            "date" => Self::Date,
            "datetime" => Self::Datetime,
            "checkbox" => Self::Checkbox,
            "select" => Self::Select,
            "multi_select" => Self::MultiSelect,
            "checkboxes" => Self::Checkboxes,
            "user" => Self::User,
            "user_multi" => Self::UserMulti,
            "labels" => Self::Labels,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Paragraph => "paragraph",
            Self::Number => "number",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Checkbox => "checkbox",
            Self::Select => "select",
            Self::MultiSelect => "multi_select",
            Self::Checkboxes => "checkboxes",
            Self::User => "user",
            Self::UserMulti => "user_multi",
            Self::Labels => "labels",
        }
    }

    pub fn has_options(self) -> bool {
        matches!(
            self,
            Self::Select | Self::MultiSelect | Self::Checkboxes | Self::Labels
        )
    }

    pub fn is_people(self) -> bool {
        matches!(self, Self::User | Self::UserMulti)
    }
}

/// Source `collectionValue`: `null` or exactly one typed key.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionValue {
    Null,
    Text(String),
    Number(f64),
    Date(NaiveDate),
    Datetime(DateTime<Utc>),
    Checkbox(bool),
    Options(Vec<Uuid>),
    Users(Vec<Uuid>),
}

impl CollectionValue {
    /// Whether the value's shape matches the field type (source `validateValue`).
    pub fn fits(&self, field_type: FieldType) -> bool {
        match self {
            Self::Null => true,
            Self::Text(_) => matches!(field_type, FieldType::Text | FieldType::Paragraph),
            Self::Number(_) => field_type == FieldType::Number,
            Self::Date(_) => field_type == FieldType::Date,
            Self::Datetime(_) => field_type == FieldType::Datetime,
            Self::Checkbox(_) => field_type == FieldType::Checkbox,
            Self::Users(_) => field_type.is_people(),
            Self::Options(_) => field_type.has_options(),
        }
    }
}

pub fn iso_millis(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn object(value: &Value) -> Parsed<&Map<String, Value>> {
    value.as_object().ok_or(InvalidInput)
}

fn only_keys(object: &Map<String, Value>, allowed: &[&str]) -> Parsed<()> {
    if object.keys().all(|key| allowed.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(InvalidInput)
    }
}

fn uuid_str(value: &Value) -> Parsed<Uuid> {
    let raw = value.as_str().ok_or(InvalidInput)?;
    if raw.len() != 36 {
        return Err(InvalidInput);
    }
    Uuid::parse_str(raw).map_err(|_| InvalidInput)
}

/// Source `z.string().trim().min(1).max(100)`: returns the trimmed name.
pub fn parse_name(value: &Value) -> Parsed<String> {
    let trimmed = value.as_str().ok_or(InvalidInput)?.trim();
    let len = trimmed.encode_utf16().count();
    if len == 0 || len > NAME_MAX {
        return Err(InvalidInput);
    }
    Ok(trimmed.to_string())
}

pub fn positive_int(value: &Value) -> Parsed<i32> {
    let number = value.as_i64().ok_or(InvalidInput)?;
    if number < 1 || number > i64::from(i32::MAX) {
        return Err(InvalidInput);
    }
    Ok(number as i32)
}

fn limited_string(value: &Value, max: usize) -> Parsed<String> {
    let raw = value.as_str().ok_or(InvalidInput)?;
    if raw.encode_utf16().count() > max {
        return Err(InvalidInput);
    }
    Ok(raw.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Document,
    Task,
}

impl CollectionKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "document" => Some(Self::Document),
            "task" => Some(Self::Task),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Task => "task",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectionCreateInput {
    pub name: String,
    pub kind: CollectionKind,
    pub project_id: Option<Uuid>,
}

pub fn parse_collection_create(body: &Value) -> Parsed<CollectionCreateInput> {
    let o = object(body)?;
    only_keys(o, &["name", "kind", "projectId"])?;
    let name = parse_name(o.get("name").ok_or(InvalidInput)?)?;
    let kind = o
        .get("kind")
        .and_then(Value::as_str)
        .and_then(CollectionKind::parse)
        .ok_or(InvalidInput)?;
    let project_id = match o.get("projectId").ok_or(InvalidInput)? {
        Value::Null => None,
        other => Some(uuid_str(other)?),
    };
    if kind == CollectionKind::Task && project_id.is_none() {
        return Err(InvalidInput);
    }
    Ok(CollectionCreateInput {
        name,
        kind,
        project_id,
    })
}

#[derive(Debug, Clone)]
pub struct FieldCreateInput {
    pub name: String,
    pub key: Option<String>,
    pub field_type: FieldType,
    pub description: Option<String>,
    pub options: Vec<String>,
}

pub fn field_key_is_valid(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|ch| matches!(ch, 'a'..='z' | '0'..='9' | '_'))
        && key.len() <= FIELD_KEY_MAX
}

pub fn parse_field_create(body: &Value) -> Parsed<FieldCreateInput> {
    let o = object(body)?;
    only_keys(o, &["name", "key", "type", "description", "options"])?;
    let name = parse_name(o.get("name").ok_or(InvalidInput)?)?;
    let key = match o.get("key") {
        None => None,
        Some(value) => {
            let raw = value.as_str().ok_or(InvalidInput)?;
            if !field_key_is_valid(raw) {
                return Err(InvalidInput);
            }
            Some(raw.to_string())
        }
    };
    let field_type = o
        .get("type")
        .and_then(Value::as_str)
        .and_then(FieldType::parse)
        .ok_or(InvalidInput)?;
    let description = match o.get("description") {
        None | Some(Value::Null) => None,
        Some(value) => Some(limited_string(value, FIELD_DESCRIPTION_MAX)?),
    };
    let options = match o.get("options") {
        None => Vec::new(),
        Some(value) => {
            let array = value.as_array().ok_or(InvalidInput)?;
            if array.len() > CREATE_OPTIONS_MAX {
                return Err(InvalidInput);
            }
            array.iter().map(parse_name).collect::<Parsed<Vec<_>>>()?
        }
    };
    if !options.is_empty() && !field_type.has_options() {
        return Err(InvalidInput);
    }
    Ok(FieldCreateInput {
        name,
        key,
        field_type,
        description,
        options,
    })
}

#[derive(Debug, Clone)]
pub struct OptionPatch {
    pub id: Option<Uuid>,
    pub label: String,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub struct FieldPatchInput {
    pub expected_version: i32,
    pub name: Option<String>,
    /// `Some(None)` clears the description.
    pub description: Option<Option<String>>,
    pub deleted: Option<bool>,
    pub options: Option<Vec<OptionPatch>>,
}

pub fn parse_field_patch(body: &Value) -> Parsed<FieldPatchInput> {
    let o = object(body)?;
    only_keys(
        o,
        &[
            "expectedVersion",
            "name",
            "description",
            "deleted",
            "options",
        ],
    )?;
    let expected_version = positive_int(o.get("expectedVersion").ok_or(InvalidInput)?)?;
    let name = o.get("name").map(parse_name).transpose()?;
    let description = match o.get("description") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(limited_string(value, FIELD_DESCRIPTION_MAX)?)),
    };
    let deleted = match o.get("deleted") {
        None => None,
        Some(value) => Some(value.as_bool().ok_or(InvalidInput)?),
    };
    let options = match o.get("options") {
        None => None,
        Some(value) => {
            let array = value.as_array().ok_or(InvalidInput)?;
            if array.len() > PATCH_OPTIONS_MAX {
                return Err(InvalidInput);
            }
            let mut out = Vec::with_capacity(array.len());
            for entry in array {
                let e = object(entry)?;
                only_keys(e, &["id", "label", "deleted"])?;
                out.push(OptionPatch {
                    id: e.get("id").map(uuid_str).transpose()?,
                    label: parse_name(e.get("label").ok_or(InvalidInput)?)?,
                    deleted: match e.get("deleted") {
                        None => false,
                        Some(flag) => flag.as_bool().ok_or(InvalidInput)?,
                    },
                });
            }
            Some(out)
        }
    };
    if name.is_none() && description.is_none() && deleted.is_none() && options.is_none() {
        return Err(InvalidInput);
    }
    Ok(FieldPatchInput {
        expected_version,
        name,
        description,
        deleted,
        options,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachTarget {
    Document(Uuid),
    Task(Uuid),
}

pub fn parse_attach(body: &Value) -> Parsed<AttachTarget> {
    let o = object(body)?;
    if o.len() != 1 {
        return Err(InvalidInput);
    }
    if let Some(value) = o.get("documentId") {
        return Ok(AttachTarget::Document(uuid_str(value)?));
    }
    if let Some(value) = o.get("taskId") {
        return Ok(AttachTarget::Task(uuid_str(value)?));
    }
    Err(InvalidInput)
}

fn uuid_list(value: &Value, max: usize) -> Parsed<Vec<Uuid>> {
    let array = value.as_array().ok_or(InvalidInput)?;
    if array.len() > max {
        return Err(InvalidInput);
    }
    array.iter().map(uuid_str).collect()
}

pub fn parse_collection_value(value: &Value) -> Parsed<CollectionValue> {
    if value.is_null() {
        return Ok(CollectionValue::Null);
    }
    let o = object(value)?;
    if o.len() != 1 {
        return Err(InvalidInput);
    }
    let (key, inner) = o.iter().next().ok_or(InvalidInput)?;
    Ok(match key.as_str() {
        "text" => CollectionValue::Text(limited_string(inner, VALUE_TEXT_MAX)?),
        "number" => {
            let number = inner.as_f64().ok_or(InvalidInput)?;
            if !number.is_finite() {
                return Err(InvalidInput);
            }
            CollectionValue::Number(number)
        }
        "date" => CollectionValue::Date(
            inner
                .as_str()
                .and_then(parse_iso_date)
                .ok_or(InvalidInput)?,
        ),
        "datetime" => CollectionValue::Datetime(
            inner
                .as_str()
                .and_then(parse_iso_datetime)
                .ok_or(InvalidInput)?,
        ),
        "checkbox" => CollectionValue::Checkbox(inner.as_bool().ok_or(InvalidInput)?),
        "options" => CollectionValue::Options(uuid_list(inner, VALUE_OPTIONS_MAX)?),
        "users" => CollectionValue::Users(uuid_list(inner, VALUE_USERS_MAX)?),
        _ => return Err(InvalidInput),
    })
}

#[derive(Debug, Clone)]
pub struct ValueInput {
    pub field_id: Uuid,
    pub expected_version: i32,
    pub expected_field_version: i32,
    pub value: CollectionValue,
}

pub fn parse_value_input(body: &Value) -> Parsed<ValueInput> {
    let o = object(body)?;
    only_keys(
        o,
        &[
            "fieldId",
            "expectedVersion",
            "expectedFieldVersion",
            "value",
        ],
    )?;
    Ok(ValueInput {
        field_id: uuid_str(o.get("fieldId").ok_or(InvalidInput)?)?,
        expected_version: positive_int(o.get("expectedVersion").ok_or(InvalidInput)?)?,
        expected_field_version: positive_int(o.get("expectedFieldVersion").ok_or(InvalidInput)?)?,
        value: parse_collection_value(o.get("value").ok_or(InvalidInput)?)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Status,
    Field(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBy {
    Due,
    Start,
    Field(Uuid),
}

/// Source `collectionQueryConfig`.
#[derive(Debug, Clone)]
pub struct QueryConfig {
    pub query: ViewQuery,
    pub group_by: Option<GroupBy>,
    pub date_by: Option<DateBy>,
}

impl QueryConfig {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "query": view_query_to_json(&self.query),
            "groupBy": match self.group_by {
                None => Value::Null,
                Some(GroupBy::Status) => Value::String("status".into()),
                Some(GroupBy::Field(id)) => Value::String(id.to_string()),
            },
            "dateBy": match self.date_by {
                None => Value::Null,
                Some(DateBy::Due) => Value::String("due".into()),
                Some(DateBy::Start) => Value::String("start".into()),
                Some(DateBy::Field(id)) => Value::String(id.to_string()),
            },
        })
    }
}

pub fn parse_query_config(value: &Value) -> Parsed<QueryConfig> {
    let o = object(value)?;
    only_keys(o, &["query", "groupBy", "dateBy"])?;
    let query = match o.get("query") {
        None => ViewQuery::default(),
        Some(value) => parse_view_query_value(value).map_err(|_| InvalidInput)?,
    };
    let group_by = match o.get("groupBy") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw == "status" => Some(GroupBy::Status),
        Some(other) => Some(GroupBy::Field(uuid_str(other)?)),
    };
    let date_by = match o.get("dateBy") {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) if raw == "due" => Some(DateBy::Due),
        Some(Value::String(raw)) if raw == "start" => Some(DateBy::Start),
        Some(other) => Some(DateBy::Field(uuid_str(other)?)),
    };
    Ok(QueryConfig {
        query,
        group_by,
        date_by,
    })
}

#[derive(Debug, Clone)]
pub struct CalendarWindow {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub time_zone: String,
}

/// `Option<Option<T>>`: outer `None` = key absent, `Some(None)` = explicit null.
#[derive(Debug, Clone)]
pub struct QueryInput {
    pub config: QueryConfig,
    pub group: Option<Option<String>>,
    pub day: Option<Option<NaiveDate>>,
    pub window: Option<CalendarWindow>,
    pub cursor: Option<String>,
    pub limit: i64,
}

pub fn parse_query_input(body: &Value) -> Parsed<QueryInput> {
    let o = object(body)?;
    only_keys(o, &["config", "group", "day", "window", "cursor", "limit"])?;
    let config = parse_query_config(o.get("config").ok_or(InvalidInput)?)?;
    let group = match o.get("group") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(limited_string(value, GROUP_MAX)?)),
    };
    let day = match o.get("day") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(value) => Some(Some(
            value
                .as_str()
                .and_then(parse_iso_date)
                .ok_or(InvalidInput)?,
        )),
    };
    let window = match o.get("window") {
        None => None,
        Some(value) => {
            let w = object(value)?;
            only_keys(w, &["from", "to", "timeZone"])?;
            let date = |key: &str| {
                w.get(key)
                    .and_then(Value::as_str)
                    .and_then(parse_iso_date)
                    .ok_or(InvalidInput)
            };
            let time_zone = w
                .get("timeZone")
                .and_then(Value::as_str)
                .ok_or(InvalidInput)?;
            if time_zone.is_empty() || time_zone.encode_utf16().count() > TIME_ZONE_MAX {
                return Err(InvalidInput);
            }
            Some(CalendarWindow {
                from: date("from")?,
                to: date("to")?,
                time_zone: time_zone.to_string(),
            })
        }
    };
    let cursor = match o.get("cursor") {
        None => None,
        Some(value) => Some(limited_string(value, CURSOR_MAX)?),
    };
    let limit = match o.get("limit") {
        None => QUERY_LIMIT_DEFAULT,
        Some(value) => {
            let limit = value.as_i64().ok_or(InvalidInput)?;
            if !(1..=QUERY_LIMIT_MAX).contains(&limit) {
                return Err(InvalidInput);
            }
            limit
        }
    };
    Ok(QueryInput {
        config,
        group,
        day,
        window,
        cursor,
        limit,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionViewType {
    Table,
    Board,
    Calendar,
}

impl CollectionViewType {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "table" => Some(Self::Table),
            "board" => Some(Self::Board),
            "calendar" => Some(Self::Calendar),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Board => "board",
            Self::Calendar => "calendar",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Shared,
}

impl Visibility {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Shared => "shared",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CollectionViewInput {
    pub name: String,
    pub view_type: CollectionViewType,
    pub visibility: Visibility,
    pub config: QueryConfig,
    /// Required on update, rejected on create.
    pub expected_version: Option<i32>,
}

pub fn parse_collection_view(body: &Value, patch: bool) -> Parsed<CollectionViewInput> {
    let o = object(body)?;
    if patch {
        only_keys(
            o,
            &["name", "type", "visibility", "config", "expectedVersion"],
        )?;
    } else {
        only_keys(o, &["name", "type", "visibility", "config"])?;
    }
    Ok(CollectionViewInput {
        name: parse_name(o.get("name").ok_or(InvalidInput)?)?,
        view_type: o
            .get("type")
            .and_then(Value::as_str)
            .and_then(CollectionViewType::parse)
            .ok_or(InvalidInput)?,
        visibility: o
            .get("visibility")
            .and_then(Value::as_str)
            .and_then(Visibility::parse)
            .ok_or(InvalidInput)?,
        config: parse_query_config(o.get("config").ok_or(InvalidInput)?)?,
        expected_version: if patch {
            Some(positive_int(o.get("expectedVersion").ok_or(InvalidInput)?)?)
        } else {
            None
        },
    })
}

pub const PROJECT_VIEW_TYPES: &[&str] = &["list", "board", "calendar", "gantt", "table"];

#[derive(Debug, Clone)]
pub struct ProjectViewCreateInput {
    pub name: String,
    pub view_type: String,
    pub config: ViewQuery,
}

pub fn parse_project_view_create(body: &Value) -> Parsed<ProjectViewCreateInput> {
    let o = object(body)?;
    only_keys(o, &["name", "type", "config"])?;
    let view_type = o.get("type").and_then(Value::as_str).ok_or(InvalidInput)?;
    if !PROJECT_VIEW_TYPES.contains(&view_type) {
        return Err(InvalidInput);
    }
    Ok(ProjectViewCreateInput {
        name: parse_name(o.get("name").ok_or(InvalidInput)?)?,
        view_type: view_type.to_string(),
        config: parse_view_query_value(o.get("config").ok_or(InvalidInput)?)
            .map_err(|_| InvalidInput)?,
    })
}

#[derive(Debug, Clone)]
pub struct ProjectViewPatchInput {
    pub name: Option<String>,
    pub config: Option<ViewQuery>,
    pub expected_config: Option<ViewQuery>,
}

/// Source `viewPatchInput`: name and/or config; a config change needs the
/// config the client last saw (`expectedConfig`, compare-and-swap).
pub fn parse_project_view_patch(body: &Value) -> Parsed<ProjectViewPatchInput> {
    let o = object(body)?;
    only_keys(o, &["name", "config", "expectedConfig"])?;
    let parse_query = |value: &Value| parse_view_query_value(value).map_err(|_| InvalidInput);
    let input = ProjectViewPatchInput {
        name: o.get("name").map(parse_name).transpose()?,
        config: o.get("config").map(parse_query).transpose()?,
        expected_config: o.get("expectedConfig").map(parse_query).transpose()?,
    };
    if input.name.is_none() && input.config.is_none() {
        return Err(InvalidInput);
    }
    if input.config.is_some() && input.expected_config.is_none() {
        return Err(InvalidInput);
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn collection_create_requires_project_for_task_kind() {
        assert!(
            parse_collection_create(&json!({"name":" A ","kind":"document","projectId":null}))
                .is_ok()
        );
        assert!(
            parse_collection_create(&json!({"name":"A","kind":"task","projectId":null})).is_err()
        );
        assert!(parse_collection_create(&json!({"name":"A","kind":"document"})).is_err());
        assert!(
            parse_collection_create(&json!({"name":"  ","kind":"document","projectId":null}))
                .is_err()
        );
        assert!(parse_collection_create(
            &json!({"name":"A","kind":"document","projectId":null,"x":1})
        )
        .is_err());
    }

    #[test]
    fn value_union_is_strict() {
        assert_eq!(
            parse_collection_value(&json!(null)).unwrap(),
            CollectionValue::Null
        );
        assert!(parse_collection_value(&json!({"text":"a","number":1})).is_err());
        assert!(parse_collection_value(&json!({"number":"1"})).is_err());
        assert!(parse_collection_value(&json!({"date":"2026-02-30"})).is_err());
        assert!(parse_collection_value(&json!({"datetime":"2026-02-01T10:00:00+09:00"})).is_err());
        assert!(parse_collection_value(&json!({"datetime":"2026-02-01T10:00Z"})).is_ok());
        assert!(parse_collection_value(&json!({"text":"x".repeat(10_001)})).is_err());
        assert!(parse_collection_value(&json!({"options":[ID]})).is_ok());
        assert!(parse_collection_value(&json!({"users":["nope"]})).is_err());
        assert!(CollectionValue::Text("a".into()).fits(FieldType::Paragraph));
        assert!(!CollectionValue::Options(vec![]).fits(FieldType::User));
    }

    #[test]
    fn field_patch_needs_a_change_and_positive_version() {
        assert!(parse_field_patch(&json!({"expectedVersion":1})).is_err());
        assert!(parse_field_patch(&json!({"expectedVersion":0,"name":"a"})).is_err());
        let patch = parse_field_patch(&json!({"expectedVersion":2,"description":null})).unwrap();
        assert_eq!(patch.description, Some(None));
        assert!(parse_field_create(&json!({"name":"n","type":"text","options":["a"]})).is_err());
        assert!(parse_field_create(&json!({"name":"n","type":"select","key":"Bad"})).is_err());
    }

    #[test]
    fn query_input_limits_and_config_defaults() {
        let parsed = parse_query_input(&json!({"config":{}})).unwrap();
        assert_eq!(parsed.limit, 50);
        assert!(parsed.config.group_by.is_none());
        assert!(parse_query_input(&json!({"config":{},"limit":101})).is_err());
        assert!(parse_query_input(&json!({"config":{"groupBy":"x"}})).is_err());
        let with_null = parse_query_input(&json!({"config":{},"group":null,"day":null})).unwrap();
        assert!(matches!(with_null.group, Some(None)));
        assert!(matches!(with_null.day, Some(None)));
        let config = parse_query_config(&json!({"groupBy":"status","dateBy":ID})).unwrap();
        let back = config.to_json();
        assert_eq!(back["groupBy"], "status");
        assert_eq!(back["dateBy"], ID);
        assert_eq!(back["query"], json!({"filters":{},"sort":[]}));
    }

    #[test]
    fn project_view_patch_requires_expected_config_for_config() {
        assert!(parse_project_view_patch(&json!({"config":{}})).is_err());
        assert!(parse_project_view_patch(&json!({"config":{},"expectedConfig":{}})).is_ok());
        assert!(parse_project_view_patch(&json!({})).is_err());
        assert!(
            parse_project_view_create(&json!({"name":"v","type":"kanban","config":{}})).is_err()
        );
    }
}
