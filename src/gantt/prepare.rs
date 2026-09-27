use std::collections::HashMap;
use uuid::Uuid;

use super::layout::{pack_flow, stack_rows};
use super::links::link_paths;
use super::scale::{make_scale, month_bands, scale_width, ticks};
use super::schedule::schedule_tasks;
use super::types::{
    GanttBarOutput, GanttLayoutItemOutput, GanttLayoutOutput, GanttLinkInput, GanttScaleOutput,
    GanttTaskInput, PackMode, ScheduledTask, ZoomLevel,
};

const MAX_DISPLAY_PATHS: usize = 2048;

pub struct PrepareInput {
    pub tasks: Vec<GanttTaskInput>,
    pub links: Vec<GanttLinkInput>,
    pub holidays: Vec<String>,
    pub scale_start: String,
    pub scale_end: String,
    pub zoom: ZoomLevel,
    pub px_per_day: Option<f64>,
    pub lane_height: i32,
    pub pack: PackMode,
    pub max_lanes: Option<i32>,
    pub truncated: bool,
    pub item_meta: Vec<GanttLayoutItemOutput>,
}

pub fn prepare_gantt(input: PrepareInput) -> GanttLayoutOutput {
    let cal = super::calendar::make_calendar(input.holidays);
    let scale = make_scale(
        &input.scale_start,
        &input.scale_end,
        input.zoom,
        input.px_per_day,
    );
    let (scheduled, dropped) = schedule_tasks(&input.tasks);
    let max_lanes = input.max_lanes.unwrap_or(i32::MAX);
    let (bars, overflow) = match input.pack {
        PackMode::Overlap => pack_flow(&scheduled, &scale, &input.links, max_lanes),
        PackMode::Rows => stack_rows(&scheduled, &scale),
    };
    let bar_ids: std::collections::HashSet<Uuid> = bars.iter().map(|b| b.id).collect();
    let drawable: Vec<GanttLinkInput> = input
        .links
        .iter()
        .filter(|l| bar_ids.contains(&l.blocker_id) && bar_ids.contains(&l.blocked_id))
        .cloned()
        .collect();
    let shown: Vec<GanttLinkInput> = if drawable.len() <= MAX_DISPLAY_PATHS {
        drawable.clone()
    } else {
        let mut sorted = drawable.clone();
        sorted.sort_by(|a, b| {
            a.blocker_id
                .cmp(&b.blocker_id)
                .then(a.blocked_id.cmp(&b.blocked_id))
        });
        sorted.truncate(MAX_DISPLAY_PATHS);
        sorted
    };
    let path_points = link_paths(&shown, &bars, input.lane_height, scale.px_per_day);
    let item_index: HashMap<Uuid, usize> = scheduled
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id, i))
        .collect();
    let mut paths: Vec<Vec<i64>> = Vec::new();
    for (i, pts) in path_points.iter().enumerate() {
        let link = &shown[i];
        let blocker = item_index.get(&link.blocker_id).unwrap();
        let blocked = item_index.get(&link.blocked_id).unwrap();
        let mut row = vec![*blocker as i64, *blocked as i64];
        for v in pts {
            row.push(v.round() as i64);
        }
        paths.push(row);
    }
    let columns = ticks(&scale, Some(&cal));
    let month_bands = month_bands(&columns);
    let lane_count = bars.iter().map(|b| b.lane + 1).max().unwrap_or(0);
    let max_link_y = path_points
        .iter()
        .flat_map(|p| p.chunks(2).map(|c| c.get(1).unwrap_or(&0.0)))
        .fold(0.0f64, |a, &y| a.max(y));
    let height = (lane_count as f64 * input.lane_height as f64)
        .max(max_link_y + 12.0)
        .max(input.lane_height as f64);
    let items = merge_items(&scheduled, &input.item_meta);
    let bars_out: Vec<GanttBarOutput> = bars
        .iter()
        .map(|b| GanttBarOutput {
            id: b.id.to_string(),
            lane: b.lane,
            x: b.x,
            width: b.width,
            milestone: b.milestone,
            inferred: b.inferred.clone(),
        })
        .collect();
    let mut bars_sorted = bars_out.clone();
    bars_sorted.sort_by(|a, b| {
        a.lane
            .cmp(&b.lane)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    GanttLayoutOutput {
        truncated: input.truncated,
        items,
        scale: GanttScaleOutput {
            zoom: scale.zoom.as_str().to_string(),
            start: scale.start.clone(),
            end: scale.end.clone(),
            px_per_day: scale.px_per_day,
        },
        lane_height: input.lane_height,
        lane_count,
        pack: input.pack.as_str().to_string(),
        bars: bars_sorted,
        path_total: drawable.len() as i32,
        paths,
        columns,
        month_bands,
        width: scale_width(&scale),
        height,
        overflow: overflow.iter().map(|id| id.to_string()).collect(),
        dropped: dropped.iter().map(|id| id.to_string()).collect(),
    }
}

fn merge_items(
    scheduled: &[ScheduledTask],
    meta: &[GanttLayoutItemOutput],
) -> Vec<GanttLayoutItemOutput> {
    let by_id: HashMap<Uuid, &GanttLayoutItemOutput> = meta
        .iter()
        .filter_map(|m| Uuid::parse_str(&m.id).ok().map(|id| (id, m)))
        .collect();
    scheduled
        .iter()
        .map(|s| {
            let base = by_id.get(&s.id).cloned();
            GanttLayoutItemOutput {
                id: s.id.to_string(),
                title: s.title.clone(),
                number: base.map(|b| b.number).unwrap_or(0),
                status_id: base.map(|b| b.status_id.clone()).unwrap_or_default(),
                priority: base
                    .map(|b| b.priority.clone())
                    .unwrap_or_else(|| "none".into()),
                assignee_ids: base.map(|b| b.assignee_ids.clone()).unwrap_or_default(),
                start_date: base.and_then(|b| b.start_date.clone()),
                due_date: base.and_then(|b| b.due_date.clone()),
                due_at: base.and_then(|b| b.due_at.clone()),
                start: s.start.clone(),
                end: s.end.clone(),
                milestone: s.milestone,
                inferred: s.inferred.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gantt::types::{GanttTaskInput, ScheduleInference};

    #[test]
    fn one_fs_path_between_two_tasks() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let tasks = vec![
            GanttTaskInput {
                id: a,
                title: "A".into(),
                start: Some("2026-09-01".into()),
                due: Some("2026-09-03".into()),
                milestone: false,
            },
            GanttTaskInput {
                id: b,
                title: "B".into(),
                start: Some("2026-09-04".into()),
                due: Some("2026-09-04".into()),
                milestone: false,
            },
        ];
        let links = vec![GanttLinkInput {
            blocker_id: a,
            blocked_id: b,
            link_type: super::super::types::LinkType::Fs,
            lag_days: 0,
        }];
        let meta = tasks
            .iter()
            .map(|t| GanttLayoutItemOutput {
                id: t.id.to_string(),
                title: t.title.clone(),
                number: 1,
                status_id: Uuid::from_u128(3).to_string(),
                priority: "none".into(),
                assignee_ids: vec![],
                start_date: t.start.clone(),
                due_date: t.due.clone(),
                due_at: None,
                start: t.start.clone().unwrap(),
                end: t.due.clone().unwrap(),
                milestone: false,
                inferred: ScheduleInference::None,
            })
            .collect();
        let out = prepare_gantt(PrepareInput {
            tasks,
            links,
            holidays: vec![],
            scale_start: "2026-09-01".into(),
            scale_end: "2026-09-30".into(),
            zoom: ZoomLevel::Day,
            px_per_day: None,
            lane_height: 36,
            pack: PackMode::Rows,
            max_lanes: None,
            truncated: false,
            item_meta: meta,
        });
        assert_eq!(out.path_total, 1);
        assert_eq!(out.paths.len(), 1);
    }
}
