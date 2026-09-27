use uuid::Uuid;

use super::date::to_epoch_day;
use super::types::{GanttTaskInput, ScheduleInference, ScheduledTask};

pub fn schedule_tasks(tasks: &[GanttTaskInput]) -> (Vec<ScheduledTask>, Vec<Uuid>) {
    let mut scheduled = Vec::new();
    let mut dropped = Vec::new();
    for t in tasks {
        match schedule_task(t) {
            Some(one) => scheduled.push(one),
            None => dropped.push(t.id),
        }
    }
    (scheduled, dropped)
}

fn schedule_task(t: &GanttTaskInput) -> Option<ScheduledTask> {
    let start = normalize(t.start.as_deref());
    let due = normalize(t.due.as_deref());

    if start.is_none() && due.is_none() {
        return None;
    }
    if let Some(s) = start.as_ref().filter(|_| due.is_none()) {
        return Some(build(t, s, s, ScheduleInference::FromStart));
    }
    if let Some(d) = due.as_ref().filter(|_| start.is_none()) {
        return Some(build(t, d, d, ScheduleInference::FromDue));
    }
    let start = start.unwrap();
    let due = due.unwrap();
    let s = to_epoch_day(&start)?;
    let e = to_epoch_day(&due)?;
    if e < s {
        return Some(build(t, &due, &start, ScheduleInference::Swapped));
    }
    Some(build(t, &start, &due, ScheduleInference::None))
}

fn build(t: &GanttTaskInput, start: &str, end: &str, inferred: ScheduleInference) -> ScheduledTask {
    ScheduledTask {
        id: t.id,
        title: t.title.clone(),
        start: start.to_string(),
        end: end.to_string(),
        milestone: t.milestone,
        inferred,
    }
}

fn normalize(v: Option<&str>) -> Option<String> {
    match v {
        None | Some("") => None,
        Some(s) if to_epoch_day(s).is_none() => None,
        Some(s) => Some(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn task(id: Uuid, start: Option<&str>, due: Option<&str>) -> GanttTaskInput {
        GanttTaskInput {
            id,
            title: "t".into(),
            start: start.map(str::to_string),
            due: due.map(str::to_string),
            milestone: false,
        }
    }

    #[test]
    fn swapped_dates() {
        let id = Uuid::from_u128(42);
        let one = schedule_task(&task(id, Some("2026-09-10"), Some("2026-08-01"))).unwrap();
        assert_eq!(one.start, "2026-08-01");
        assert_eq!(one.end, "2026-09-10");
        assert_eq!(one.inferred, ScheduleInference::Swapped);
    }
}
