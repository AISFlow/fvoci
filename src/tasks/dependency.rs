use chrono::{Datelike, NaiveDate, Weekday};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyType {
    Fs,
    Ss,
    Ff,
}

impl DependencyType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fs => "FS",
            Self::Ss => "SS",
            Self::Ff => "FF",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "FS" => Some(Self::Fs),
            "SS" => Some(Self::Ss),
            "FF" => Some(Self::Ff),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleEnds {
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
}

pub fn finish_date(
    due_date: Option<NaiveDate>,
    due_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<NaiveDate> {
    due_date.or_else(|| due_at.map(|value| value.date_naive()))
}

pub fn schedule_ends(
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    due_at: Option<chrono::DateTime<chrono::Utc>>,
) -> ScheduleEnds {
    ScheduleEnds {
        start_date,
        due_date: finish_date(due_date, due_at),
    }
}

pub fn required_dates_present(
    dependency_type: DependencyType,
    blocker: ScheduleEnds,
    blocked: ScheduleEnds,
) -> bool {
    match dependency_type {
        DependencyType::Fs => blocker.due_date.is_some() && blocked.start_date.is_some(),
        DependencyType::Ss => blocker.start_date.is_some() && blocked.start_date.is_some(),
        DependencyType::Ff => blocker.due_date.is_some() && blocked.due_date.is_some(),
    }
}

fn add_calendar_days(date: NaiveDate, days: i32) -> NaiveDate {
    let mut cur = date;
    for _ in 0..days {
        cur = cur.succ_opt().unwrap_or(cur);
    }
    cur
}

fn is_working_day(date: NaiveDate, holidays: &HashSet<NaiveDate>) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && !holidays.contains(&date)
}

fn add_working_days(date: NaiveDate, days: i32, holidays: &HashSet<NaiveDate>) -> NaiveDate {
    if days == 0 {
        return date;
    }
    let mut remaining = days;
    let mut cur = date;
    let mut guard = 0;
    while remaining > 0 {
        cur = add_calendar_days(cur, 1);
        if is_working_day(cur, holidays) {
            remaining -= 1;
        }
        guard += 1;
        if guard > 800 {
            return cur;
        }
    }
    cur
}

pub fn violates_inequality(
    dependency_type: DependencyType,
    lag_days: i32,
    blocker: ScheduleEnds,
    blocked: ScheduleEnds,
    holidays: &HashSet<NaiveDate>,
) -> bool {
    match dependency_type {
        DependencyType::Fs => {
            let (Some(due), Some(start)) = (blocker.due_date, blocked.start_date) else {
                return false;
            };
            start < add_working_days(due, lag_days, holidays)
        }
        DependencyType::Ss => {
            let (Some(a), Some(b)) = (blocker.start_date, blocked.start_date) else {
                return false;
            };
            b < add_working_days(a, lag_days, holidays)
        }
        DependencyType::Ff => {
            let (Some(due_a), Some(due_b)) = (blocker.due_date, blocked.due_date) else {
                return false;
            };
            due_b < add_working_days(due_a, lag_days, holidays)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_same_day_is_ok_and_earlier_start_contradicts() {
        let holidays = HashSet::new();
        let blocker = ScheduleEnds {
            start_date: None,
            due_date: Some(NaiveDate::from_ymd_opt(2031, 4, 10).unwrap()),
        };
        let ok = ScheduleEnds {
            start_date: Some(NaiveDate::from_ymd_opt(2031, 4, 10).unwrap()),
            due_date: None,
        };
        let bad = ScheduleEnds {
            start_date: Some(NaiveDate::from_ymd_opt(2031, 4, 1).unwrap()),
            due_date: None,
        };
        assert!(required_dates_present(DependencyType::Fs, blocker, ok));
        assert!(!violates_inequality(
            DependencyType::Fs,
            0,
            blocker,
            ok,
            &holidays
        ));
        assert!(violates_inequality(
            DependencyType::Fs,
            0,
            blocker,
            bad,
            &holidays
        ));
    }

    #[test]
    fn missing_dates_are_not_contradictions() {
        let holidays = HashSet::new();
        let blocker = ScheduleEnds {
            start_date: None,
            due_date: Some(NaiveDate::from_ymd_opt(2031, 4, 10).unwrap()),
        };
        let blocked = ScheduleEnds {
            start_date: None,
            due_date: None,
        };
        assert!(!required_dates_present(
            DependencyType::Fs,
            blocker,
            blocked
        ));
        assert!(!violates_inequality(
            DependencyType::Fs,
            0,
            blocker,
            blocked,
            &holidays
        ));
    }

    #[test]
    fn lag_skips_weekends() {
        let holidays = HashSet::new();
        // Friday + 1 working day = Monday
        let friday = NaiveDate::from_ymd_opt(2031, 4, 11).unwrap();
        assert_eq!(friday.weekday(), Weekday::Fri);
        let monday = add_working_days(friday, 1, &holidays);
        assert_eq!(monday, NaiveDate::from_ymd_opt(2031, 4, 14).unwrap());
    }
}
