use chrono::NaiveDate;

use super::types::IsoDate;

/// Gantt epoch math only: accepts proleptic Gregorian dates chrono can represent,
/// including year 0 padding (e.g. `0000-12-31`). User/PG task dates still use
/// `crate::tasks::parse_iso_date`, which rejects year 0000.
fn parse_gantt_date(value: &str) -> Option<NaiveDate> {
    let bytes = value.as_bytes();
    let shape_ok = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shape_ok {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

pub fn to_epoch_day(d: &str) -> Option<i32> {
    let date = parse_gantt_date(d)?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some((date - epoch).num_days() as i32)
}

pub fn from_epoch_day(day: i32) -> Option<IsoDate> {
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
    let date = epoch + chrono::Duration::days(day as i64);
    Some(date.format("%Y-%m-%d").to_string())
}

pub fn days_between(a: &str, b: &str) -> Option<i32> {
    let x = to_epoch_day(a)?;
    let y = to_epoch_day(b)?;
    Some(y - x)
}

pub fn add_days(d: &str, n: i32) -> Option<IsoDate> {
    let day = to_epoch_day(d)?;
    from_epoch_day(day + n)
}

pub fn day_of_week(d: &str) -> Option<u8> {
    let day = to_epoch_day(d)?;
    Some((((day + 4) % 7) + 7) as u8 % 7)
}

pub fn each_day(start: &str, end: &str) -> Vec<IsoDate> {
    let s = to_epoch_day(start);
    let e = to_epoch_day(end);
    match (s, e) {
        (Some(s), Some(e)) if e >= s => (s..=e).filter_map(from_epoch_day).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_epoch_day() {
        let d = "2026-09-15";
        let day = to_epoch_day(d).unwrap();
        assert_eq!(from_epoch_day(day).as_deref(), Some(d));
    }

    #[test]
    fn days_between_inclusive_span() {
        assert_eq!(days_between("2026-09-01", "2026-09-03"), Some(2));
    }

    #[test]
    fn year_zero_padding_day_is_valid_for_gantt_math() {
        let day = to_epoch_day("0000-12-31").expect("year-0 padding date");
        assert_eq!(from_epoch_day(day).as_deref(), Some("0000-12-31"));
    }
}
