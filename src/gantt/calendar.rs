use super::date::{add_days, day_of_week, each_day};
use super::types::{IsoDate, WorkCalendar};

pub fn make_calendar(holidays: impl IntoIterator<Item = IsoDate>) -> super::types::WorkCalendar {
    super::types::WorkCalendar {
        weekend: vec![0, 6],
        holidays: holidays.into_iter().collect(),
    }
}

pub fn is_working_day(d: &str, cal: &WorkCalendar) -> bool {
    let Some(dow) = day_of_week(d) else {
        return false;
    };
    if cal.weekend.contains(&dow) {
        return false;
    }
    !cal.holidays.contains(d)
}

fn is_all_off_duty(start: &str, days: i32, cal: &WorkCalendar) -> bool {
    let Some(end) = add_days(start, days - 1) else {
        return false;
    };
    let all = each_day(start, &end);
    !all.is_empty() && all.iter().all(|d| !is_working_day(d, cal))
}

pub fn off_duty_for_tick(start: &str, days: i32, cal: &WorkCalendar) -> bool {
    is_all_off_duty(start, days, cal)
}
