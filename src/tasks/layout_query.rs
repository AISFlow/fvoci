use crate::gantt::{PackMode, ZoomLevel};
use crate::tasks::list_query::{parse_task_list_query, TaskListQueryError};

#[derive(Debug, Clone)]
pub struct ParsedTaskLayoutQuery {
    pub year: i32,
    pub month: i32,
    pub week_starts_on: u8,
    pub zoom: ZoomLevel,
    pub px_per_day: Option<f64>,
    pub lane_height: i32,
    pub pack: PackMode,
    pub max_lanes: Option<i32>,
    pub view_query_json: Option<String>,
}

pub enum TaskLayoutQueryError {
    InvalidInput,
}

pub fn parse_task_layout_query(
    raw: &std::collections::HashMap<String, String>,
) -> Result<ParsedTaskLayoutQuery, TaskLayoutQueryError> {
    let year = parse_i32(raw.get("year"), 1, 9998)?;
    let month = parse_i32(raw.get("month"), 1, 12)?;
    let week_starts_on = match raw.get("weekStartsOn").map(|s| s.as_str()) {
        None | Some("0") => 0,
        Some("1") => 1,
        _ => return Err(TaskLayoutQueryError::InvalidInput),
    };
    let zoom = raw
        .get("zoom")
        .map(|s| s.as_str())
        .unwrap_or("day");
    let zoom = ZoomLevel::parse(zoom).ok_or(TaskLayoutQueryError::InvalidInput)?;
    let px_per_day = match raw.get("pxPerDay") {
        None => None,
        Some(v) => {
            let n: f64 = v.parse().map_err(|_| TaskLayoutQueryError::InvalidInput)?;
            if !(n > 0.0 && n <= 128.0) {
                return Err(TaskLayoutQueryError::InvalidInput);
            }
            Some(n)
        }
    };
    let lane_height = match raw.get("laneHeight") {
        None => 36,
        Some(v) => {
            let n: i32 = v.parse().map_err(|_| TaskLayoutQueryError::InvalidInput)?;
            if !(16..=128).contains(&n) {
                return Err(TaskLayoutQueryError::InvalidInput);
            }
            n
        }
    };
    let pack = raw
        .get("pack")
        .map(|s| s.as_str())
        .unwrap_or("rows");
    let pack = PackMode::parse(pack).ok_or(TaskLayoutQueryError::InvalidInput)?;
    let max_lanes = match raw.get("maxLanes") {
        None => None,
        Some(v) => {
            let n: i32 = v.parse().map_err(|_| TaskLayoutQueryError::InvalidInput)?;
            if !(1..=500).contains(&n) {
                return Err(TaskLayoutQueryError::InvalidInput);
            }
            Some(n)
        }
    };
    let view_query_json = raw.get("query").map(|s| s.to_string());
    if let Some(q) = &view_query_json {
        if q.len() > 16_000 {
            return Err(TaskLayoutQueryError::InvalidInput);
        }
    }
    Ok(ParsedTaskLayoutQuery {
        year,
        month,
        week_starts_on,
        zoom,
        px_per_day,
        lane_height,
        pack,
        max_lanes,
        view_query_json,
    })
}

fn parse_i32(raw: Option<&String>, min: i32, max: i32) -> Result<i32, TaskLayoutQueryError> {
    let v = raw.ok_or(TaskLayoutQueryError::InvalidInput)?;
    let n: i32 = v.parse().map_err(|_| TaskLayoutQueryError::InvalidInput)?;
    if n < min || n > max {
        return Err(TaskLayoutQueryError::InvalidInput);
    }
    Ok(n)
}

pub fn layout_list_query(
    layout: &ParsedTaskLayoutQuery,
    from: &str,
    to: &str,
) -> Result<crate::tasks::list_query::ParsedTaskListQuery, TaskListQueryError> {
    parse_task_list_query(
        layout.view_query_json.as_deref(),
        None,
        None,
        Some(500),
        Some(from),
        Some(to),
    )
}
