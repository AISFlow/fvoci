//! Strict query-string extraction (`#[serde(deny_unknown_fields)]`) for HTTP handlers.

use std::num::IntErrorKind;
use std::ops::RangeInclusive;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fvoci_model::DuplicateAs;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::TRUE;
use crate::app::{App, ChangeNotification, DEFAULT_PER_PAGE, ListPage, MAX_PER_PAGE};
use crate::error::AppError;

const MAX_PAGE: u32 = i32::MAX.unsigned_abs();

/// Extracts and validates the request URI's query string into a typed `T` (whose
/// `#[serde(deny_unknown_fields)]` rejects unknown query parameters with 400).
#[derive(Debug, Clone, Default)]
pub(crate) struct QueryParams<T>(pub(crate) T);

impl<T> FromRequestParts<App> for QueryParams<T>
where
    T: DeserializeOwned + Send,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        _app: &App,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let map = parse_query_map(parts.uri.query());
        std::future::ready(
            serde_json::from_value::<T>(Value::Object(map))
                .map(Self)
                .map_err(|error| {
                    let text = error.to_string();
                    if let Some(unknown) = extract_unknown_field(&text) {
                        AppError::bad_request(format_args!("unknown query parameter `{unknown}`"))
                            .with_parameter(unknown)
                    } else {
                        AppError::bad_request(error)
                    }
                }),
        )
    }
}

fn extract_unknown_field(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("unknown field `")?;
    let (name, _) = rest.split_once('`')?;
    Some(name)
}

/// Decodes a raw URI query string into a map of first-seen percent-decoded string values.
pub(crate) fn parse_query_map(query: Option<&str>) -> Map<String, Value> {
    let mut map = Map::new();
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return map;
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_key, raw_val) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode_query(raw_key);
        if !key.is_empty() && !map.contains_key(&key) {
            map.insert(key, Value::String(percent_decode_query(raw_val)));
        }
    }
    map
}

fn percent_decode_query(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (byte, escaped) {
            (_, Some(value)) => {
                decoded.push(value);
                index += 3;
            }
            (b'+', None) => {
                decoded.push(b' ');
                index += 1;
            }
            (other, None) => {
                decoded.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Standard `page` and `perPage` query parameters.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PagingQuery {
    pub(crate) page: String,
    pub(crate) per_page: String,
}

impl PagingQuery {
    pub(crate) fn into_list_page(self) -> Result<ListPage, AppError> {
        paging_from_strings(&self.page, &self.per_page)
    }
}

/// Standard `disableNotify` query parameter on block/card write routes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DisableNotifyQuery {
    pub(crate) disable_notify: String,
}

impl DisableNotifyQuery {
    pub(crate) fn notification(&self) -> ChangeNotification {
        if self.disable_notify == TRUE {
            ChangeNotification::Suppress
        } else {
            ChangeNotification::Send
        }
    }
}

/// Standard `asTemplate` query parameter on duplication routes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AsTemplateQuery {
    pub(crate) as_template: String,
}

impl AsTemplateQuery {
    pub(crate) fn duplicate_as(&self) -> DuplicateAs {
        if self.as_template == TRUE {
            DuplicateAs::Template
        } else {
            DuplicateAs::Copy
        }
    }
}

/// Non-empty trimmed/raw option helper: empty string is `None`.
pub(crate) fn non_empty_opt(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// Validates `page` and `perPage` strings, reporting both errors when both are invalid.
pub(crate) fn paging_from_strings(page: &str, per_page: &str) -> Result<ListPage, AppError> {
    let page_res = count_param(page, "page", 0, 0..=MAX_PAGE);
    let per_page_res = count_param(
        per_page,
        "perPage",
        DEFAULT_PER_PAGE.get(),
        1..=MAX_PER_PAGE.get(),
    );
    match (page_res, per_page_res) {
        (Ok(p), Ok(pp)) => Ok(ListPage {
            page: p,
            per_page: pp,
        }),
        (Err(refusal), Ok(_)) | (Ok(_), Err(refusal)) => Err(refusal),
        (Err(page_err), Err(limit_err)) => Err(page_err.and(limit_err)),
    }
}

/// A count of the query parameter `name` (`default` if it is empty) within `range`,
/// as the type `T` holds it.
fn count_param<T: TryFrom<u32>>(
    text: &str,
    name: &str,
    default: u32,
    range: RangeInclusive<u32>,
) -> Result<T, AppError> {
    let number = int_param(text, name, i64::from(default))?;
    u32::try_from(number)
        .ok()
        .filter(|count| range.contains(count))
        .and_then(|count| T::try_from(count).ok())
        .ok_or_else(|| {
            let reason = if number < i64::from(*range.start()) {
                format!("below {}", range.start())
            } else {
                format!("above {}", range.end())
            };
            AppError::bad_request(format_args!("invalid `{name}` parameter: {reason}"))
                .with_parameter(name)
        })
}

/// Parses an integer query parameter value; empty string yields `default`.
pub(crate) fn int_param(text: &str, name: &str, default: i64) -> Result<i64, AppError> {
    if text.is_empty() {
        return Ok(default);
    }
    text.parse().map_err(|error: std::num::ParseIntError| {
        let reason = match error.kind() {
            IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => "out of range",
            _ => "not a whole number",
        };
        AppError::bad_request(format_args!("invalid `{name}` parameter: {reason}"))
            .with_parameter(name)
    })
}
