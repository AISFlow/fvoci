pub mod list_query;
pub mod patch;

/// Parses a calendar date exactly like the source `z.iso.date()`: `YYYY-MM-DD`
/// with a four-digit year and zero-padded month/day. chrono alone would also
/// accept `2026-1-5` and extended years such as `+262142-12-31`.
pub fn parse_iso_date(value: &str) -> Option<chrono::NaiveDate> {
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
    // Year 0000 matches the regex shape but PostgreSQL has no year zero.
    if &value[..4] == "0000" {
        return None;
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

pub fn title_is_valid(title: &str) -> bool {
    let trimmed = title.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 500
}

pub fn task_type_is_valid(value: &str) -> bool {
    matches!(value, "task" | "bug" | "story" | "epic" | "subtask")
}

pub fn priority_is_valid(value: &str) -> bool {
    matches!(value, "none" | "low" | "medium" | "high" | "urgent")
}

#[cfg(test)]
mod iso_date_tests {
    use super::parse_iso_date;

    #[test]
    fn accepts_only_zero_padded_four_digit_dates() {
        assert!(parse_iso_date("2026-01-31").is_some());
        assert!(parse_iso_date("9999-12-31").is_some());
        for bad in [
            "2026-1-5",
            "+262142-12-31",
            "2026-02-30",
            "20260131",
            "2026-01-31T00:00:00Z",
            "",
        ] {
            assert!(parse_iso_date(bad).is_none(), "{bad}");
        }
    }
}
