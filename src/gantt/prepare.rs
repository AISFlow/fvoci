use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::date::to_epoch_day;
use super::layout::{pack_flow, stack_rows};
use super::links::link_paths;
use super::scale::{make_scale, month_bands, scale_width, ticks};
use super::schedule::schedule_tasks;
use super::types::{
    GanttBarOutput, GanttCalendarOutput, GanttLayoutItemOutput, GanttLayoutOutput, GanttLinkInput,
    GanttLinkOutput, GanttScaleOutput, GanttTaskInput, IsoDate, PackMode, ScheduledTask, TimeScale,
    WorkCalendar, ZoomLevel,
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
    pub can_edit: bool,
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
    // `links` follows `items`, not `bars`: overlap packing can leave an item
    // without a bar, and a client that lays out its own bars needs them all.
    let item_ids: HashSet<Uuid> = scheduled.iter().map(|t| t.id).collect();
    let (item_links, link_total) = cap_links(
        input
            .links
            .iter()
            .filter(|l| item_ids.contains(&l.blocker_id) && item_ids.contains(&l.blocked_id))
            .cloned()
            .collect(),
    );
    let bar_ids: HashSet<Uuid> = bars.iter().map(|b| b.id).collect();
    let (shown, path_total) = cap_links(
        input
            .links
            .iter()
            .filter(|l| bar_ids.contains(&l.blocker_id) && bar_ids.contains(&l.blocked_id))
            .cloned()
            .collect(),
    );
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
    let calendar = GanttCalendarOutput {
        weekend: cal.weekend.clone(),
        holidays: holidays_in_scale(&cal, &scale),
    };
    GanttLayoutOutput {
        truncated: input.truncated,
        items,
        can_edit: input.can_edit,
        links: item_links
            .into_iter()
            .map(|l| GanttLinkOutput {
                blocker_id: l.blocker_id.to_string(),
                blocked_id: l.blocked_id.to_string(),
                link_type: l.link_type,
                lag_days: l.lag_days,
            })
            .collect(),
        link_total: link_total as i32,
        calendar,
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
        path_total: path_total as i32,
        paths,
        columns,
        month_bands,
        width: scale_width(&scale),
        height,
        overflow: overflow.iter().map(|id| id.to_string()).collect(),
        dropped: dropped.iter().map(|id| id.to_string()).collect(),
    }
}

/// At most [`MAX_DISPLAY_PATHS`] of `links`, plus how many there were. 500
/// tasks can carry far more dependency rows than a response should hold. Under
/// the cap the order is kept; over it the lowest `(blocker, blocked)` pairs are
/// kept, so the subset does not depend on the row order of the query.
fn cap_links(mut links: Vec<GanttLinkInput>) -> (Vec<GanttLinkInput>, usize) {
    let total = links.len();
    if total > MAX_DISPLAY_PATHS {
        links.sort_by(|a, b| {
            a.blocker_id
                .cmp(&b.blocker_id)
                .then(a.blocked_id.cmp(&b.blocked_id))
        });
        links.truncate(MAX_DISPLAY_PATHS);
    }
    (links, total)
}

/// The holidays of `cal` within `scale.start..=scale.end`, ascending. Every
/// kept date parsed as `YYYY-MM-DD`, so string order is date order.
fn holidays_in_scale(cal: &WorkCalendar, scale: &TimeScale) -> Vec<IsoDate> {
    let (Some(start), Some(end)) = (to_epoch_day(&scale.start), to_epoch_day(&scale.end)) else {
        return Vec::new();
    };
    let mut days: Vec<IsoDate> = cal
        .holidays
        .iter()
        .filter(|d| to_epoch_day(d).is_some_and(|day| (start..=end).contains(&day)))
        .cloned()
        .collect();
    days.sort();
    days
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
    use crate::gantt::types::{GanttTaskInput, LinkType, ScheduleInference};

    fn task(id: Uuid, start: &str, due: &str) -> GanttTaskInput {
        GanttTaskInput {
            id,
            title: id.to_string(),
            start: Some(start.into()),
            due: Some(due.into()),
            milestone: false,
        }
    }

    fn september(
        tasks: Vec<GanttTaskInput>,
        links: Vec<GanttLinkInput>,
        pack: PackMode,
        max_lanes: Option<i32>,
    ) -> GanttLayoutOutput {
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
        prepare_gantt(PrepareInput {
            tasks,
            links,
            holidays: vec![],
            scale_start: "2026-09-01".into(),
            scale_end: "2026-09-30".into(),
            zoom: ZoomLevel::Day,
            px_per_day: None,
            lane_height: 36,
            pack,
            max_lanes,
            truncated: false,
            item_meta: meta,
            can_edit: true,
        })
    }

    fn fs(blocker_id: Uuid, blocked_id: Uuid) -> GanttLinkInput {
        GanttLinkInput {
            blocker_id,
            blocked_id,
            link_type: LinkType::Fs,
            lag_days: 0,
        }
    }

    #[test]
    fn one_fs_path_between_two_tasks() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let out = september(
            vec![
                task(a, "2026-09-01", "2026-09-03"),
                task(b, "2026-09-04", "2026-09-04"),
            ],
            vec![fs(a, b)],
            PackMode::Rows,
            None,
        );
        assert_eq!(out.path_total, 1);
        assert_eq!(out.paths.len(), 1);
    }

    #[test]
    fn links_keep_items_that_overlap_packing_left_without_a_bar() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let out = september(
            vec![
                task(a, "2026-09-01", "2026-09-03"),
                task(b, "2026-09-02", "2026-09-04"),
            ],
            vec![fs(a, b)],
            PackMode::Overlap,
            Some(1),
        );
        assert_eq!(out.overflow, vec![b.to_string()]);
        assert_eq!(out.items.len(), 2);
        assert_eq!(out.path_total, 0);
        assert_eq!(out.link_total, 1);
        assert_eq!(out.links.len(), 1);
        assert_eq!(out.links[0].blocked_id, b.to_string());
    }
}
