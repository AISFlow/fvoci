//! Notion CSV database parser and column type inference.
//!
//! Infers fvoci card property definitions (`select`, `multiSelect`, `date`, `checkbox`,
//! `number`, `url`, `text`) from Notion CSV export column values and produces validated
//! property values conforming to `fvoci_model::card_property` and `fvoci_model::date_value`.

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    CardPropertyDefinition, JsonMap, PropertyColor, PropertyOption, PropertyType, check_date_value,
    new_id,
};
use serde_json::{Map, Value};

use super::markdown::is_allowed_href;
use super::summary::BundleLimits;
use crate::error::{AppError, Reason};

/// Rotating option colors assigned to inferred `select` / `multiSelect` columns.
const OPTION_COLORS: [PropertyColor; 9] = [
    PropertyColor::Blue,
    PropertyColor::Green,
    PropertyColor::Orange,
    PropertyColor::Purple,
    PropertyColor::Yellow,
    PropertyColor::Pink,
    PropertyColor::Gray,
    PropertyColor::Brown,
    PropertyColor::Red,
];

/// Inferred property kind for one non-title column of a Notion CSV database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InferredKind {
    Checkbox,
    Number,
    Date,
    Url,
    MultiSelect,
    Select,
    Text,
}

impl InferredKind {
    pub(crate) const fn property_type(self) -> PropertyType {
        match self {
            Self::Checkbox => PropertyType::Checkbox,
            Self::Number => PropertyType::Number,
            Self::Date => PropertyType::Date,
            Self::Url => PropertyType::Url,
            Self::MultiSelect => PropertyType::MultiSelect,
            Self::Select => PropertyType::Select,
            Self::Text => PropertyType::Text,
        }
    }
}

/// One parsed data row of a Notion CSV database.
#[derive(Debug)]
pub(crate) struct ParsedCsvRow {
    pub(crate) title: String,
    pub(crate) properties: JsonMap,
}

/// Complete parsed CSV database ready to turn into a fvoci Board.
#[derive(Debug)]
pub(crate) struct ParsedCsvDatabase {
    pub(crate) card_properties: Vec<CardPropertyDefinition>,
    pub(crate) all_property_ids: Vec<String>,
    pub(crate) first_select: Option<(String, Vec<String>)>,
    pub(crate) rows: Vec<ParsedCsvRow>,
}

/// Parses a Notion CSV database string and infers fvoci card property definitions,
/// enforcing both per-CSV limits (`limits.max_csv_rows`, `limits.max_csv_columns`) and
/// the caller's remaining bundle-wide row budget (`max_rows`).
///
/// # Errors
///
/// Returns 400 if the CSV is malformed or contains U+0000, or 413 if row or column count
/// exceeds `max_rows` / `limits.max_csv_rows` / `limits.max_csv_columns`.
pub(crate) fn parse_notion_csv_bounded(
    raw: &str,
    limits: &BundleLimits,
    max_rows: usize,
) -> Result<ParsedCsvDatabase, AppError> {
    if raw.contains('\0') {
        return Err(AppError::bad_request("CSV input contains U+0000 character"));
    }
    let clean = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .from_reader(clean.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|error| AppError::bad_request(format_args!("invalid CSV header: {error}")))?
        .iter()
        .map(|header| header.trim().to_owned())
        .collect();
    if headers.is_empty() {
        return Ok(ParsedCsvDatabase {
            card_properties: Vec::new(),
            all_property_ids: Vec::new(),
            first_select: None,
            rows: Vec::new(),
        });
    }
    if headers.len() > limits.max_csv_columns {
        let limit = u64::try_from(limits.max_csv_columns).unwrap_or(u64::MAX);
        return Err(AppError::too_large(
            format_args!(
                "CSV database has more than {} columns",
                limits.max_csv_columns
            ),
            limit,
        )
        .with_reason(Reason::ArchiveTooLarge));
    }

    let row_cap = limits.max_csv_rows.min(max_rows);
    let mut raw_rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record
            .map_err(|error| AppError::bad_request(format_args!("invalid CSV record: {error}")))?;
        if record.len() > limits.max_csv_columns {
            let limit = u64::try_from(limits.max_csv_columns).unwrap_or(u64::MAX);
            return Err(AppError::too_large(
                format_args!("CSV row has more than {} columns", limits.max_csv_columns),
                limit,
            )
            .with_reason(Reason::ArchiveTooLarge));
        }
        if record.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        if raw_rows.len() >= row_cap {
            let limit = u64::try_from(row_cap).unwrap_or(u64::MAX);
            return Err(AppError::too_large(
                format_args!("CSV database exceeds row limit of {row_cap}"),
                limit,
            )
            .with_reason(Reason::ArchiveTooLarge));
        }
        let cells: Vec<String> = record.iter().map(|cell| cell.trim().to_owned()).collect();
        raw_rows.push(cells);
    }

    Ok(build_database(&headers, &raw_rows))
}

#[cfg(test)]
pub(crate) fn parse_notion_csv(
    raw: &str,
    limits: &BundleLimits,
) -> Result<ParsedCsvDatabase, AppError> {
    parse_notion_csv_bounded(raw, limits, limits.max_csv_rows)
}

struct ColumnPlan {
    prop_id: String,
    kind: InferredKind,
    options: HashMap<String, String>,
    option_ids: Vec<String>,
    definition: CardPropertyDefinition,
}

fn build_database(headers: &[String], raw_rows: &[Vec<String>]) -> ParsedCsvDatabase {
    let mut plans = Vec::with_capacity(headers.len().saturating_sub(1));
    for (col_index, header) in headers.iter().enumerate().skip(1) {
        let values: Vec<&str> = raw_rows
            .iter()
            .filter_map(|row| row.get(col_index).map(String::as_str))
            .filter(|value| !value.is_empty())
            .collect();
        let col_name = if header.is_empty() {
            format!("Column {col_index}")
        } else {
            header.clone()
        };
        let kind = infer_column_kind(&col_name, &values);
        plans.push(build_column_plan(&col_name, kind, &values));
    }

    let mut rows = Vec::with_capacity(raw_rows.len());
    for raw_row in raw_rows {
        let title = raw_row
            .first()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .unwrap_or("Untitled")
            .to_owned();
        let mut properties = JsonMap::new();
        for (plan_idx, plan) in plans.iter().enumerate() {
            let cell = raw_row.get(plan_idx + 1).map_or("", String::as_str).trim();
            if cell.is_empty() {
                continue;
            }
            if let Some(value) = convert_cell(cell, plan) {
                properties.insert(plan.prop_id.clone(), value);
            }
        }
        rows.push(ParsedCsvRow { title, properties });
    }

    let first_select = plans
        .iter()
        .find(|plan| plan.kind == InferredKind::Select)
        .map(|plan| (plan.prop_id.clone(), plan.option_ids.clone()));
    let all_property_ids = plans.iter().map(|plan| plan.prop_id.clone()).collect();
    let card_properties = plans.into_iter().map(|plan| plan.definition).collect();
    ParsedCsvDatabase {
        card_properties,
        all_property_ids,
        first_select,
        rows,
    }
}

fn build_column_plan(name: &str, kind: InferredKind, values: &[&str]) -> ColumnPlan {
    let prop_id = new_id();
    let mut options_map = HashMap::new();
    let mut option_ids = Vec::new();
    let mut options_list = Vec::new();

    if matches!(kind, InferredKind::Select | InferredKind::MultiSelect) {
        let tokens: Vec<&str> = match kind {
            InferredKind::Select => values.to_vec(),
            InferredKind::MultiSelect => values
                .iter()
                .flat_map(|cell| cell.split(',').map(str::trim))
                .filter(|token| !token.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        for token in tokens {
            if !options_map.contains_key(token) {
                let opt_id = new_id();
                let color = OPTION_COLORS[options_list.len() % OPTION_COLORS.len()];
                options_map.insert(token.to_owned(), opt_id.clone());
                option_ids.push(opt_id.clone());
                options_list.push(PropertyOption::new(opt_id, token, color));
            }
        }
    }

    let definition = CardPropertyDefinition {
        id: prop_id.clone(),
        name: name.to_owned(),
        property_type: kind.property_type(),
        options: options_list,
    };
    ColumnPlan {
        prop_id,
        kind,
        options: options_map,
        option_ids,
        definition,
    }
}

/// Infers the most specific fvoci property type matching all non-empty `values` in a column.
pub(crate) fn infer_column_kind(header: &str, values: &[&str]) -> InferredKind {
    if values.is_empty() {
        return InferredKind::Text;
    }
    if values.iter().all(|value| parse_checkbox(value).is_some()) {
        return InferredKind::Checkbox;
    }
    if values.iter().all(|value| parse_number(value).is_some()) {
        return InferredKind::Number;
    }
    if values
        .iter()
        .all(|value| parse_notion_date(value).is_some())
    {
        return InferredKind::Date;
    }
    if values.iter().all(|value| is_http_url(value)) {
        return InferredKind::Url;
    }
    if looks_like_multi_select(values) {
        return InferredKind::MultiSelect;
    }
    if looks_like_select(header, values) {
        return InferredKind::Select;
    }
    InferredKind::Text
}

fn convert_cell(cell: &str, plan: &ColumnPlan) -> Option<Value> {
    match plan.kind {
        InferredKind::Checkbox => parse_checkbox(cell)
            .filter(|&checked| checked)
            .map(|_| Value::String("true".to_owned())),
        InferredKind::Number => parse_number(cell).map(Value::String),
        InferredKind::Date => parse_notion_date(cell).map(Value::Object),
        InferredKind::Url | InferredKind::Text => Some(Value::String(cell.to_owned())),
        InferredKind::Select => plan.options.get(cell).cloned().map(Value::String),
        InferredKind::MultiSelect => {
            let mut seen = HashSet::new();
            let ids: Vec<Value> = cell
                .split(',')
                .map(str::trim)
                .filter_map(|token| plan.options.get(token))
                .filter(|opt_id| seen.insert(opt_id.as_str()))
                .map(|opt_id| Value::String(opt_id.clone()))
                .collect();
            (!ids.is_empty()).then_some(Value::Array(ids))
        }
    }
}

fn parse_checkbox(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "checked" | "☑" | "✓" => Some(true),
        "no" | "false" | "unchecked" | "☐" | "✗" => Some(false),
        _ => None,
    }
}

fn parse_number(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix(['$', '€', '£', '₩', '¥'])
        .unwrap_or(trimmed);
    let stripped = stripped.strip_suffix('%').unwrap_or(stripped).trim();
    if stripped.is_empty() {
        return None;
    }
    let normalized = if stripped.contains(',') {
        if !is_thousands_grouped_number(stripped) {
            return None;
        }
        stripped.replace(',', "")
    } else {
        stripped.to_owned()
    };
    let parsed: f64 = normalized.parse().ok()?;
    parsed.is_finite().then_some(normalized)
}

/// Whether `text` uses strict English thousands grouping (`1,234` / `-1,234,567.89`),
/// avoiding misclassifying European decimals (`1,5`) or comma-separated lists (`1,2,3`).
fn is_thousands_grouped_number(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (integer, fractional) = match unsigned.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (unsigned, None),
    };
    if let Some(frac) = fractional
        && (frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let mut groups = integer.split(',');
    let Some(first) = groups.next() else {
        return false;
    };
    if first.is_empty() || first.len() > 3 || !first.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut has_tail = false;
    for group in groups {
        has_tail = true;
        if group.len() != 3 || !group.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    has_tail
}

fn is_http_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !trimmed.chars().any(char::is_whitespace)
        && is_allowed_href(trimmed)
}

fn looks_like_multi_select(values: &[&str]) -> bool {
    if values.iter().any(|value| value.contains('\n')) {
        return false;
    }
    let mut has_multi = false;
    let mut distinct = HashSet::new();
    for value in values {
        let tokens: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .collect();
        if tokens.is_empty() || tokens.len() > 10 {
            return false;
        }
        if tokens.len() >= 2 {
            has_multi = true;
        }
        for token in tokens {
            if token.chars().count() > 40
                || token.split_whitespace().count() > 4
                || token.ends_with('.')
            {
                return false;
            }
            distinct.insert(token);
        }
    }
    has_multi && distinct.len() <= 30
}

fn looks_like_select(header: &str, values: &[&str]) -> bool {
    if values
        .iter()
        .any(|value| value.contains([',', '\n']) || value.chars().count() > 40)
    {
        return false;
    }
    if values
        .iter()
        .any(|value| value.split_whitespace().count() > 4 || value.ends_with('.'))
    {
        return false;
    }
    let distinct: HashSet<&str> = values.iter().copied().collect();
    if distinct.is_empty() || distinct.len() > 20 {
        return false;
    }
    let repeats = distinct.len() < values.len();
    let lower = header.trim().to_ascii_lowercase();
    let select_header = matches!(
        lower.as_str(),
        "status"
            | "stage"
            | "priority"
            | "category"
            | "type"
            | "state"
            | "phase"
            | "상태"
            | "우선순위"
            | "분류"
    );
    repeats || select_header || (values.len() >= 2 && distinct.len() <= 10)
}

/// Parses a Notion CSV date cell (single day or range) into a validated fvoci date object.
pub(crate) fn parse_notion_date(raw: &str) -> Option<JsonMap> {
    let trimmed = raw.trim();
    let range = trimmed.split_once('→').or_else(|| trimmed.split_once("->"));
    let mut map = Map::new();
    if let Some((start, end)) = range {
        let from = parse_single_day(start)?;
        let to = parse_single_day(end)?;
        map.insert("from".to_owned(), Value::String(from.clone()));
        if to > from {
            map.insert("to".to_owned(), Value::String(to));
        }
    } else {
        let from = parse_single_day(trimmed)?;
        map.insert("from".to_owned(), Value::String(from));
    }
    check_date_value(&map).ok()?;
    Some(map)
}

fn parse_single_day(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ts) = trimmed.parse::<jiff::Timestamp>() {
        return Some(jiff::tz::TimeZone::UTC.to_datetime(ts).date().to_string());
    }
    // Strip optional trailing timezone label like "(UTC)" or "(KST)".
    let without_tz = trimmed.split('(').next().unwrap_or(trimmed).trim();
    // YYYY-MM-DD or YYYY/MM/DD (with optional time tail)
    let first_word = without_tz.split(['T', ' ']).next().unwrap_or("");
    if first_word.len() == 10 {
        let normalized = first_word.replace('/', "-");
        if is_valid_day(&normalized) {
            return Some(normalized);
        }
    }
    parse_english_month_day(without_tz)
}

fn parse_english_month_day(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    let month_str = parts.next()?;
    let day_str = parts.next()?.trim_end_matches(',');
    let year_str = parts.next()?.trim_end_matches(',');
    let month = month_number(month_str)?;
    let day: u32 = day_str.parse().ok()?;
    let year: u32 = year_str.parse().ok()?;
    let formatted = format!("{year:04}-{month:02}-{day:02}");
    is_valid_day(&formatted).then_some(formatted)
}

fn month_number(word: &str) -> Option<u32> {
    match word.to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn is_valid_day(day: &str) -> bool {
    let mut map = Map::new();
    map.insert("from".to_owned(), Value::String(day.to_owned()));
    check_date_value(&map).is_ok()
}
