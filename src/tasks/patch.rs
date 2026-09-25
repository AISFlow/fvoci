use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use uuid::Uuid;

pub fn estimate_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 19 {
        return false;
    }
    let dot = bytes.iter().position(|&b| b == b'.');
    match dot {
        None => bytes.len() <= 12 && bytes.iter().all(|b| b.is_ascii_digit()),
        Some(idx) => {
            let int_part = &bytes[..idx];
            let frac_part = &bytes[idx + 1..];
            !int_part.is_empty()
                && int_part.len() <= 12
                && !frac_part.is_empty()
                && frac_part.len() <= 6
                && int_part.iter().all(|b| b.is_ascii_digit())
                && frac_part.iter().all(|b| b.is_ascii_digit())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExpectedDatesInput {
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct PatchTaskMetaInput {
    pub task_type: Option<String>,
    pub title: Option<String>,
    pub priority: Option<String>,
    pub status_id: Option<Uuid>,
    pub start_date: FieldUpdate<NaiveDate>,
    pub due_date: FieldUpdate<NaiveDate>,
    pub due_at: FieldUpdate<DateTime<Utc>>,
    pub estimate: FieldUpdate<String>,
    pub parent_id: FieldUpdate<Uuid>,
    pub recurrence: FieldUpdate<Value>,
    pub archived: Option<bool>,
    pub expected_dates: Option<ExpectedDatesInput>,
    pub assignee_ids: Option<Vec<Uuid>>,
    pub label_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Default)]
pub struct MoveTaskInput {
    pub status_id: Uuid,
    pub expected_status_id: Option<Uuid>,
    pub before_id: Option<Uuid>,
    pub after_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldUpdate<T> {
    #[default]
    Unchanged,
    Set(T),
    Clear,
}

impl<T> FieldUpdate<T> {
    pub fn from_optional(value: Option<Option<T>>) -> Self {
        match value {
            None => Self::Unchanged,
            Some(None) => Self::Clear,
            Some(Some(value)) => Self::Set(value),
        }
    }
}
