use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUser {
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

pub fn as_text_scale(value: i16) -> i16 {
    if matches!(value, 16 | 18 | 20) {
        value
    } else {
        16
    }
}

pub fn as_week_starts_on(value: i32) -> i32 {
    if matches!(value, 0 | 1) {
        value
    } else {
        1
    }
}
