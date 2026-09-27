use super::layout::lane_center_y;
use super::types::{GanttLinkInput, LaidOutBar, LinkType};

const ELBOW_PAD: f64 = 10.0;
const DETOUR_STEP: f64 = 6.0;

#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

pub fn link_paths(
    links: &[GanttLinkInput],
    bars: &[LaidOutBar],
    lane_height: i32,
    px_per_day: f64,
) -> Vec<Vec<f64>> {
    let by_id: std::collections::HashMap<_, _> = bars.iter().map(|b| (b.id, b)).collect();
    let mut items = Vec::new();
    for l in links {
        let (from, to) = match (by_id.get(&l.blocker_id), by_id.get(&l.blocked_id)) {
            (Some(f), Some(t)) => (f, t),
            _ => continue,
        };
        let lag = l.lag_days as f64 * px_per_day;
        let (x1, x2) = match l.link_type {
            LinkType::Ss => (from.x, to.x + lag),
            LinkType::Ff => (from.x + from.width, to.x + to.width + lag),
            LinkType::Fs => (from.x + from.width, to.x + lag),
        };
        let y1 = lane_center_y(from.lane, lane_height);
        let y2 = lane_center_y(to.lane, lane_height);
        items.push((from, to, x1, x2, y1, y2));
    }
    let mut used: Vec<(f64, f64, f64)> = Vec::new();
    items
        .into_iter()
        .map(|(from, to, x1, x2, y1, y2)| {
            elbow(from, to, x1, x2, y1, y2, bars, lane_height, &mut used)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn elbow(
    from: &LaidOutBar,
    to: &LaidOutBar,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    bars: &[LaidOutBar],
    lane_height: i32,
    used: &mut Vec<(f64, f64, f64)>,
) -> Vec<f64> {
    let overlap_l = from.x.max(to.x);
    let overlap_r = (from.x + from.width).min(to.x + to.width);
    let pts = if overlap_r > overlap_l && y1 != y2 {
        let cx = (overlap_l + overlap_r) / 2.0;
        let edge = lane_height as f64 * 0.26;
        vec![
            Point {
                x: cx,
                y: if y1 < y2 { y1 + edge } else { y1 - edge },
            },
            Point {
                x: cx,
                y: if y2 < y1 { y2 + edge } else { y2 - edge },
            },
        ]
    } else {
        let gap = x2 - x1;
        if gap >= ELBOW_PAD * 2.0 {
            let mid = (x1 + x2) / 2.0;
            vec![
                Point { x: x1, y: y1 },
                Point { x: mid, y: y1 },
                Point { x: mid, y: y2 },
                Point { x: x2, y: y2 },
            ]
        } else if gap >= 0.0 {
            if y1 == y2 {
                vec![Point { x: x1, y: y1 }, Point { x: x2, y: y2 }]
            } else {
                let mid = (x1 + x2) / 2.0;
                vec![
                    Point { x: x1, y: y1 },
                    Point { x: mid, y: y1 },
                    Point { x: mid, y: y2 },
                    Point { x: x2, y: y2 },
                ]
            }
        } else {
            let left = x2 - ELBOW_PAD;
            let right = x1 + ELBOW_PAD;
            let detour_y = pick_gutter(left, right, y1, y2, bars, lane_height, used);
            vec![
                Point { x: x1, y: y1 },
                Point {
                    x: x1 + ELBOW_PAD,
                    y: y1,
                },
                Point {
                    x: x1 + ELBOW_PAD,
                    y: detour_y,
                },
                Point {
                    x: x2 - ELBOW_PAD,
                    y: detour_y,
                },
                Point {
                    x: x2 - ELBOW_PAD,
                    y: y2,
                },
                Point { x: x2, y: y2 },
            ]
        }
    };
    pts.iter().flat_map(|p| [p.x, p.y]).collect()
}

fn pick_gutter(
    left: f64,
    right: f64,
    y1: f64,
    y2: f64,
    bars: &[LaidOutBar],
    lane_height: i32,
    used: &mut Vec<(f64, f64, f64)>,
) -> f64 {
    let lo = y1.min(y2);
    let hi = y1.max(y2);
    let mut prefs = Vec::new();
    if y1 != y2 {
        prefs.push((y1 + y2) / 2.0);
    }
    let gutter = (lo / lane_height as f64).floor() as i32 + 1;
    let gutter_y = gutter as f64 * lane_height as f64;
    if gutter_y > lo && gutter_y < hi {
        prefs.push(gutter_y);
    }
    prefs.push(lo - lane_height as f64 * 0.38);
    prefs.push(hi + lane_height as f64 * 0.38);
    let deltas = [
        0.0,
        -DETOUR_STEP,
        DETOUR_STEP,
        -DETOUR_STEP * 2.0,
        DETOUR_STEP * 2.0,
    ];
    let mut seen: Vec<i64> = Vec::new();
    for base in prefs {
        let key = (base * 1000.0) as i64;
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        for d in deltas {
            let y = base + d;
            if y < 4.0 {
                continue;
            }
            if hits_name(y, left, right, bars, lane_height) {
                continue;
            }
            if used
                .iter()
                .any(|(uy, ul, ur)| (uy - y).abs() < DETOUR_STEP - 1.0 && left < *ur && right > *ul)
            {
                continue;
            }
            used.push((y, left, right));
            return y;
        }
    }
    let fallback = hi + lane_height as f64 * 0.38;
    used.push((fallback, left, right));
    fallback
}

fn hits_name(y: f64, left: f64, right: f64, bars: &[LaidOutBar], lane_height: i32) -> bool {
    let half = lane_height as f64 * 0.32;
    for b in bars {
        let cy = lane_center_y(b.lane, lane_height);
        if y < cy - half || y > cy + half {
            continue;
        }
        let x0 = b.x;
        let x1 = b.x + b.width + if b.milestone { 64.0 } else { 0.0 };
        if left < x1 && right > x0 {
            return true;
        }
    }
    false
}
