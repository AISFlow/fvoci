//! The time of a signed request, in the two forms the providers read: the date of
//! HTTP (`Thu, 05 Jan 2014 21:31:40 GMT`) and AWS's basic ISO 8601 (`20150830T123600Z`).

use std::time::{SystemTime, UNIX_EPOCH};

/// A moment, in whole seconds since the Unix epoch, broken into its UTC date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Moment {
    year: i64,
    month: u32,
    day: u32,
    hour: u64,
    minute: u64,
    second: u64,
    /// 0 for Sunday.
    weekday: usize,
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Moment {
    /// Now, by the system's clock; a clock before 1970 reads as the epoch.
    pub(super) fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self::at(seconds)
    }

    /// The moment `seconds` after the Unix epoch.
    pub(super) fn at(seconds: u64) -> Self {
        let millis = i64::try_from(seconds)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        let (year, month, day) = civil_from_millis(millis);
        let of_day = seconds % 86_400;
        Self {
            year,
            month,
            day,
            hour: of_day / 3600,
            minute: of_day % 3600 / 60,
            second: of_day % 60,
            // 1970-01-01 was a Thursday.
            weekday: usize::try_from((seconds / 86_400 + 4) % 7).expect("a weekday is 0 to 6"),
        }
    }

    /// `Thu, 05 Jan 2014 21:31:40 GMT` (RFC 7231's IMF-fixdate).
    pub(super) fn http_date(self) -> String {
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
            WEEKDAYS[self.weekday],
            self.day,
            MONTHS[usize::try_from(self.month - 1).expect("a month is 1 to 12")],
            self.year,
            self.hour,
            self.minute,
            self.second
        )
    }

    /// `20150830T123600Z`, as `X-Amz-Date` gives it.
    pub(super) fn amz_date(self) -> String {
        format!(
            "{}T{:02}{:02}{:02}Z",
            self.amz_day(),
            self.hour,
            self.minute,
            self.second
        )
    }

    /// `20150830`, the day of a signature's scope.
    pub(super) fn amz_day(self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }
}

/// The year, month and day (UTC) of a time in milliseconds since the Unix epoch.
///
/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_millis(millis: i64) -> (i64, u32, u32) {
    let days = millis.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_millis_matches_known_dates() {
        assert_eq!(civil_from_millis(0), (1970, 1, 1), "the Unix epoch");
        // 2026-11-10T05:15:39Z.
        assert_eq!(
            civil_from_millis(1_794_287_739_000),
            (2026, 11, 10),
            "a date of this year"
        );
    }

    #[test]
    fn a_moment_reads_as_the_dates_of_the_published_examples() {
        // The time of AWS's signature test suite: 2015-08-30T12:36:00Z.
        let aws = Moment::at(1_440_938_160);
        assert_eq!(aws.amz_date(), "20150830T123600Z");
        assert_eq!(aws.amz_day(), "20150830");
        // The time of OCI's signing examples (which call the day a Thursday).
        assert_eq!(
            Moment::at(1_388_957_500).http_date(),
            "Sun, 05 Jan 2014 21:31:40 GMT"
        );
        assert_eq!(Moment::at(0).http_date(), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(Moment::at(0).amz_date(), "19700101T000000Z");
        // A leap day, and the end of a year.
        assert_eq!(Moment::at(1_709_208_000).amz_date(), "20240229T120000Z");
        assert_eq!(Moment::at(1_735_689_599).amz_date(), "20241231T235959Z");
        assert_eq!(
            Moment::at(1_735_689_599).http_date(),
            "Tue, 31 Dec 2024 23:59:59 GMT"
        );
    }
}
