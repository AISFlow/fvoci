use super::date::{add_days, day_of_week, to_epoch_day};
use super::types::IsoDate;

/// Week-aligned calendar range covering a month (source `monthRange`).
pub fn month_range(year: i32, month: i32, week_starts_on: u8) -> Option<(IsoDate, IsoDate)> {
    if !(1..=12).contains(&month) || !(1..=9998).contains(&year) {
        return None;
    }
    let first = format!("{year:04}-{month:02}-01");
    let first_day = to_epoch_day(&first)?;
    let first_dow = day_of_week(&first)?;
    let lead = ((first_dow as i32 - week_starts_on as i32) % 7 + 7) % 7;
    let start = add_days(&first, -lead)?;
    let next_month_first = if month == 12 {
        format!("{year:04}-01-01")
    } else {
        format!("{year:04}-{:02}-01", month + 1)
    };
    let last_day = to_epoch_day(&next_month_first)?;
    let days = ((last_day - (first_day - lead)) as f64 / 7.0).ceil() as i32 * 7;
    let end = add_days(&start, days - 1)?;
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn september_2026_has_week_padding() {
        let (start, end) = month_range(2026, 9, 0).unwrap();
        assert!(start.as_str() < "2026-09-01");
        assert!(end.as_str() >= "2026-09-30");
    }
}
