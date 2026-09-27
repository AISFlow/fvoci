use super::calendar::off_duty_for_tick;
use super::date::{add_days, days_between, to_epoch_day};
use super::types::{IsoDate, ScaleTickOutput, TimeScale, WorkCalendar, ZoomLevel};

const DEFAULT_PX: [(ZoomLevel, f64); 4] = [
    (ZoomLevel::Day, 32.0),
    (ZoomLevel::Week, 12.0),
    (ZoomLevel::Month, 4.0),
    (ZoomLevel::Quarter, 1.5),
];

pub fn make_scale(start: &str, end: &str, zoom: ZoomLevel, px_per_day: Option<f64>) -> TimeScale {
    let px = px_per_day.unwrap_or_else(|| {
        DEFAULT_PX
            .iter()
            .find(|(z, _)| *z == zoom)
            .map(|(_, p)| *p)
            .unwrap_or(32.0)
    });
    let span = days_between(start, end);
    let safe_end = match span {
        None => start.to_string(),
        Some(s) if s < 0 => start.to_string(),
        Some(_) => end.to_string(),
    };
    TimeScale {
        zoom,
        start: start.to_string(),
        end: safe_end,
        px_per_day: px,
    }
}

pub fn date_to_x(d: &str, scale: &TimeScale) -> Option<f64> {
    let offset = days_between(&scale.start, d)?;
    Some(offset as f64 * scale.px_per_day)
}

pub fn scale_width(scale: &TimeScale) -> f64 {
    let span = days_between(&scale.start, &scale.end).unwrap_or(0);
    (span + 1) as f64 * scale.px_per_day
}

pub fn ticks(scale: &TimeScale, cal: Option<&WorkCalendar>) -> Vec<ScaleTickOutput> {
    let start_day = to_epoch_day(&scale.start);
    let end_day = to_epoch_day(&scale.end);
    let (start_day, end_day) = match (start_day, end_day) {
        (Some(s), Some(e)) if e >= s => (s, e),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut cursor = start_day;
    while cursor <= end_day {
        let date = super::date::from_epoch_day(cursor).unwrap_or_default();
        let next_boundary = next_tick_start(&date, scale.zoom);
        let boundary_day = next_boundary
            .and_then(|d| to_epoch_day(&d))
            .unwrap_or(end_day + 1);
        let stop = boundary_day.min(end_day + 1);
        let days = stop - cursor;
        if days <= 0 {
            break;
        }
        let x = (cursor - start_day) as f64 * scale.px_per_day;
        let off_duty = cal
            .map(|c| off_duty_for_tick(&date, days, c))
            .unwrap_or(false);
        out.push(ScaleTickOutput {
            date: date.clone(),
            x,
            width: days as f64 * scale.px_per_day,
            label: tick_label(&date, scale.zoom),
            off_duty,
        });
        cursor = stop;
    }
    out
}

pub fn month_bands(cols: &[ScaleTickOutput]) -> Vec<super::types::MonthBandOutput> {
    let mut out: Vec<super::types::MonthBandOutput> = Vec::new();
    for c in cols {
        let key = c.date.get(0..7).unwrap_or(&c.date).to_string();
        if let Some(last) = out.last_mut() {
            if last.key == key {
                last.width += c.width;
                continue;
            }
        }
        let month = c.date.get(5..7).and_then(|m| m.parse::<u32>().ok());
        let label = month
            .map(|m| format!("{m}월"))
            .unwrap_or_else(|| key.clone());
        out.push(super::types::MonthBandOutput {
            key,
            label,
            x: c.x,
            width: c.width,
        });
    }
    out
}

fn next_tick_start(d: &str, zoom: ZoomLevel) -> Option<IsoDate> {
    match zoom {
        ZoomLevel::Day => add_days(d, 1),
        ZoomLevel::Week => {
            let dow = day_of_week_or_zero(d);
            let to_monday = if dow == 0 { 1 } else { 8 - dow };
            add_days(d, to_monday)
        }
        ZoomLevel::Month | ZoomLevel::Quarter => {
            let y = d.get(0..4).and_then(|s| s.parse::<i32>().ok())?;
            let m = d.get(5..7).and_then(|s| s.parse::<i32>().ok())?;
            let step = if zoom == ZoomLevel::Month {
                1
            } else {
                3 - ((m - 1) % 3)
            };
            let nm = m + step;
            let ny = y + (nm - 1).div_euclid(12);
            let nmm = (nm - 1).rem_euclid(12) + 1;
            Some(format!("{ny:04}-{nmm:02}-01"))
        }
    }
}

fn day_of_week_or_zero(d: &str) -> i32 {
    super::date::day_of_week(d).unwrap_or(0) as i32
}

fn tick_label(d: &str, zoom: ZoomLevel) -> String {
    let y = d.get(0..4).unwrap_or("");
    let m = d.get(5..7).unwrap_or("");
    let day = d.get(8..10).unwrap_or("");
    match zoom {
        ZoomLevel::Day => day.trim_start_matches('0').to_string(),
        ZoomLevel::Week => format!("{}/{}", m, day),
        ZoomLevel::Month => format!("{}-{}", y, m),
        ZoomLevel::Quarter => {
            let mm = m.parse::<i32>().unwrap_or(0);
            let q = (mm - 1) / 3 + 1;
            format!("{} Q{}", y, q)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gantt::calendar::make_calendar;

    #[test]
    fn korean_month_band_label() {
        let scale = make_scale("2026-09-01", "2026-09-30", ZoomLevel::Day, None);
        let cols = ticks(&scale, None);
        let bands = month_bands(&cols);
        assert!(bands.iter().any(|b| b.label == "9월"));
    }

    #[test]
    fn holiday_column_off_duty() {
        let scale = make_scale("2026-09-01", "2026-09-05", ZoomLevel::Day, None);
        let cal = make_calendar(["2026-09-02".into()]);
        let cols = ticks(&scale, Some(&cal));
        let sep2 = cols.iter().find(|c| c.date == "2026-09-02").unwrap();
        assert!(sep2.off_duty);
    }
}
