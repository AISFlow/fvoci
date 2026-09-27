use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::date::days_between;
use super::scale::date_to_x;
use super::types::{GanttLinkInput, LaidOutBar, ScheduledTask, TimeScale};

const MILESTONE_WIDTH: f64 = 12.0;
const PACK_GAP: f64 = 1.0;

struct BarRect {
    x: f64,
    width: f64,
}

pub fn pack_lanes(
    tasks: &[ScheduledTask],
    scale: &TimeScale,
    max_lanes: i32,
) -> (Vec<LaidOutBar>, Vec<Uuid>) {
    let sorted = sort_for_packing(tasks);
    let mut lane_end: Vec<f64> = Vec::new();
    let mut bars = Vec::new();
    let mut overflow = Vec::new();
    for t in sorted {
        let rect = bar_rect(t, scale);
        if rect.is_none() {
            overflow.push(t.id);
            continue;
        }
        let rect = rect.unwrap();
        let lane = first_free_lane(&lane_end, rect.x, 0);
        if lane >= max_lanes {
            overflow.push(t.id);
            continue;
        }
        if lane_end.len() <= lane as usize {
            lane_end.resize(lane as usize + 1, 0.0);
        }
        lane_end[lane as usize] = rect.x + rect.width;
        bars.push(LaidOutBar {
            id: t.id,
            lane,
            x: rect.x,
            width: rect.width,
            milestone: t.milestone,
            inferred: t.inferred.clone(),
        });
    }
    (bars, overflow)
}

pub fn pack_flow(
    tasks: &[ScheduledTask],
    scale: &TimeScale,
    links: &[GanttLinkInput],
    max_lanes: i32,
) -> (Vec<LaidOutBar>, Vec<Uuid>) {
    let order = order_by_dependency(tasks, links);
    let mut preds: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for l in links {
        preds.entry(l.blocked_id).or_default().push(l.blocker_id);
    }
    let mut lane_end: Vec<f64> = Vec::new();
    let mut placed: HashMap<Uuid, LaidOutBar> = HashMap::new();
    let mut bars = Vec::new();
    let mut overflow = Vec::new();
    for t in order {
        let rect = bar_rect(&t, scale);
        if rect.is_none() {
            overflow.push(t.id);
            continue;
        }
        let rect = rect.unwrap();
        let mut min_lane = 0;
        for pid in preds.get(&t.id).unwrap_or(&Vec::new()) {
            if let Some(pb) = placed.get(pid) {
                if ranges_overlap(pb, &rect) {
                    min_lane = min_lane.max(pb.lane + 1);
                } else {
                    min_lane = min_lane.max(pb.lane);
                }
            }
        }
        let lane = first_free_lane(&lane_end, rect.x, min_lane);
        if lane >= max_lanes {
            overflow.push(t.id);
            continue;
        }
        if lane_end.len() <= lane as usize {
            lane_end.resize(lane as usize + 1, 0.0);
        }
        lane_end[lane as usize] = rect.x + rect.width;
        let bar = LaidOutBar {
            id: t.id,
            lane,
            x: rect.x,
            width: rect.width,
            milestone: t.milestone,
            inferred: t.inferred.clone(),
        };
        placed.insert(t.id, bar.clone());
        bars.push(bar);
    }
    (bars, overflow)
}

pub fn stack_rows(tasks: &[ScheduledTask], scale: &TimeScale) -> (Vec<LaidOutBar>, Vec<Uuid>) {
    let sorted = sort_for_packing(tasks);
    let mut bars = Vec::new();
    let mut overflow = Vec::new();
    let mut lane = 0;
    for t in sorted {
        let rect = bar_rect(t, scale);
        if rect.is_none() {
            overflow.push(t.id);
            continue;
        }
        let rect = rect.unwrap();
        bars.push(LaidOutBar {
            id: t.id,
            lane,
            x: rect.x,
            width: rect.width,
            milestone: t.milestone,
            inferred: t.inferred.clone(),
        });
        lane += 1;
    }
    (bars, overflow)
}

pub fn bar_rect(t: &ScheduledTask, scale: &TimeScale) -> Option<BarRect> {
    let x = date_to_x(&t.start, scale)?;
    if t.milestone {
        return Some(BarRect {
            x: x + scale.px_per_day / 2.0 - MILESTONE_WIDTH / 2.0,
            width: MILESTONE_WIDTH,
        });
    }
    let span = days_between(&t.start, &t.end)?;
    Some(BarRect {
        x,
        width: (span + 1) as f64 * scale.px_per_day,
    })
}

pub fn lane_center_y(lane: i32, lane_height: i32) -> f64 {
    lane as f64 * lane_height as f64 + lane_height as f64 / 2.0
}

fn first_free_lane(lane_end: &[f64], x: f64, min_lane: i32) -> i32 {
    for i in min_lane..lane_end.len() as i32 {
        let end = lane_end[i as usize];
        if end + PACK_GAP <= x {
            return i;
        }
    }
    lane_end.len().max(min_lane as usize) as i32
}

fn ranges_overlap(a: &LaidOutBar, b: &BarRect) -> bool {
    a.x < b.x + b.width + PACK_GAP && b.x < a.x + a.width + PACK_GAP
}

fn order_by_dependency(
    tasks: &[ScheduledTask],
    links: &[GanttLinkInput],
) -> Vec<ScheduledTask> {
    let by_id: HashMap<Uuid, ScheduledTask> = tasks.iter().map(|t| (t.id, t.clone())).collect();
    let mut indeg: HashMap<Uuid, i32> = tasks.iter().map(|t| (t.id, 0)).collect();
    let mut adj: HashMap<Uuid, Vec<Uuid>> = tasks.iter().map(|t| (t.id, Vec::new())).collect();
    for l in links {
        if !by_id.contains_key(&l.blocker_id) || !by_id.contains_key(&l.blocked_id) {
            continue;
        }
        adj.get_mut(&l.blocker_id).unwrap().push(l.blocked_id);
        indeg.insert(l.blocked_id, indeg.get(&l.blocked_id).unwrap_or(&0) + 1);
    }
    let mut ready: Vec<ScheduledTask> = tasks
        .iter()
        .filter(|t| indeg.get(&t.id).unwrap_or(&0) == &0)
        .cloned()
        .collect();
    ready.sort_by(cmp_packing);
    let mut out = Vec::new();
    let mut deg = indeg.clone();
    while !ready.is_empty() {
        let t = ready.remove(0);
        let tid = t.id;
        out.push(t);
        for n in adj.get(&tid).cloned().unwrap_or_default() {
            let next = deg.get(&n).unwrap_or(&1) - 1;
            deg.insert(n, next);
            if next == 0 {
                if let Some(nt) = by_id.get(&n) {
                    ready.push(nt.clone());
                    ready.sort_by(cmp_packing);
                }
            }
        }
    }
    if out.len() < tasks.len() {
        let seen: HashSet<Uuid> = out.iter().map(|t| t.id).collect();
        let mut rest: Vec<ScheduledTask> = tasks
            .iter()
            .filter(|t| !seen.contains(&t.id))
            .cloned()
            .collect();
        rest.sort_by(cmp_packing);
        out.extend(rest);
    }
    out
}

fn sort_for_packing(tasks: &[ScheduledTask]) -> Vec<&ScheduledTask> {
    let mut out: Vec<&ScheduledTask> = tasks.iter().collect();
    out.sort_by(|a, b| cmp_packing(a, b));
    out
}

fn cmp_packing(a: &ScheduledTask, b: &ScheduledTask) -> std::cmp::Ordering {
    match a.start.cmp(&b.start) {
        std::cmp::Ordering::Equal => a.id.cmp(&b.id),
        other => other,
    }
}
