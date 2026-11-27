//! Whom the server writes for: the language it writes in for them, of those it writes
//! its own text in, and the day it is for them.

use fvoci_model::UserSettings;

use super::App;
use super::language::Language;
use crate::error::AppError;

/// Whoever the server writes something for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reader {
    /// The language the server writes in for them ([`App::language_for`]).
    pub(super) language: Language,
    /// The calendar day it is for them, as `YYYY-MM-DD`: in the time zone of their
    /// settings, or in UTC where those leave the zone to the device, which the server
    /// does not know. `None` at a time outside years 1 to 9999.
    pub(super) today: Option<String>,
}

impl App {
    /// The user as somebody the server writes for at the time `now`, in milliseconds,
    /// by `accept_language` (the request's `Accept-Language`) where their settings name
    /// no language of the server's.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the settings cannot be read.
    pub(super) async fn reader_at(
        &self,
        user_id: &str,
        accept_language: Option<&str>,
        now: i64,
    ) -> Result<Reader, AppError> {
        let settings = self.user_settings(user_id).await?;
        Ok(Reader {
            language: language_of(&settings, accept_language),
            today: fvoci_model::day_at(now, &settings.time_zone),
        })
    }

    /// The language the server writes in for a user: the titles of a copy, the built-in
    /// templates they are offered and their welcome board. It is the language of their
    /// settings; where those leave it to the device, or name a language the server
    /// writes nothing in, the first language of the server's that `accept_language`
    /// (the request's `Accept-Language`) prefers; failing both, English.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the settings cannot be read.
    pub(super) async fn language_for(
        &self,
        user_id: &str,
        accept_language: Option<&str>,
    ) -> Result<Language, AppError> {
        let settings = self.user_settings(user_id).await?;
        Ok(language_of(&settings, accept_language))
    }
}

/// The language of the settings, or else the one `accept_language` prefers, or else
/// English.
fn language_of(settings: &UserSettings, accept_language: Option<&str>) -> Language {
    Language::first_of([
        Language::written_in(settings.language.as_str()),
        accept_language.and_then(accepted_language),
    ])
}

/// The language an `Accept-Language` header prefers among those the server writes in:
/// the ranges are tried from the highest `q` down, as they stand in the header for the
/// same `q`, and the first that names a language of [`Language`] gives it. `*` is the
/// first of [`Language::ALL`]. A range with `q=0`, or with a `q` that is no number from
/// 0 to 1, is none.
///
/// `None` for a header that names no language of the server's.
fn accepted_language(header: &str) -> Option<Language> {
    let mut ranges: Vec<(&str, f32)> = header.split(',').filter_map(range_of).collect();
    // Stable: ranges of the same weight stay in the order the header gives them.
    ranges.sort_by(|(_, one), (_, other)| other.total_cmp(one));
    ranges.into_iter().find_map(|(tag, _)| {
        if tag == "*" {
            Language::ALL.first().copied()
        } else {
            Language::written_in(tag)
        }
    })
}

/// One range of the header and its weight: `ko-KR;q=0.8` is `ko-KR` at 0.8, and a
/// range without `q` weighs 1. `None` for a blank one and for one that is not accepted.
fn range_of(range: &str) -> Option<(&str, f32)> {
    let mut members = range.split(';').map(str::trim);
    let tag = members.next().filter(|tag| !tag.is_empty())?;
    let weight = match members.find_map(|member| {
        let (name, value) = member.split_once('=')?;
        name.trim().eq_ignore_ascii_case("q").then(|| value.trim())
    }) {
        Some(value) => value.parse::<f32>().ok()?,
        None => 1.0,
    };
    (weight > 0.0 && weight <= 1.0).then_some((tag, weight))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(header: &str) -> Option<Language> {
        accepted_language(header)
    }

    #[test]
    fn the_first_language_the_server_writes_in_is_accepted() {
        assert_eq!(accepted("ko-KR,ko;q=0.9,en;q=0.8"), Some(Language::Korean));
        assert_eq!(accepted("en-GB, ko;q=0.5"), Some(Language::English));
        assert_eq!(
            accepted("ja, de;q=0.9, KO;q=0.2"),
            Some(Language::Korean),
            "the languages before it are none of the server's"
        );
    }

    #[test]
    fn the_weights_order_the_ranges_and_the_header_orders_equal_ones() {
        assert_eq!(accepted("en;q=0.3, ko;q=0.8"), Some(Language::Korean));
        assert_eq!(accepted("en;q=0.8, ko;q=0.8"), Some(Language::English));
        assert_eq!(accepted("ko ; Q = 0.4 , en;q=0.5"), Some(Language::English));
    }

    #[test]
    fn a_range_that_is_refused_or_malformed_is_none() {
        assert_eq!(accepted("ko;q=0, en;q=0.1"), Some(Language::English));
        assert_eq!(accepted("ko;q=0"), None);
        assert_eq!(accepted("ko;q=much, ja"), None);
        assert_eq!(accepted("ko;q=1.5"), None);
        assert_eq!(accepted("ko;q=NaN"), None);
        assert_eq!(accepted(""), None);
        assert_eq!(accepted(" , ;q=1"), None);
    }

    #[test]
    fn any_language_is_the_first_the_server_writes_in() {
        assert_eq!(accepted("ja, *;q=0.1"), Some(Language::English));
        assert_eq!(accepted("*, ko;q=0.9"), Some(Language::English));
        assert_eq!(accepted("ko;q=0.9, *;q=0.5"), Some(Language::Korean));
    }
}
