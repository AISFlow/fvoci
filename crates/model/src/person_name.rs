//! A person's name: its parts as they write it and in Latin letters, the name they
//! wrote to be shown by, which of these they are shown by, and their title and
//! credentials. The one rule of the name a person is shown by is here
//! ([`PersonName::shown`]); the web app's preview passes the same vectors
//! (`contracts/name-vectors.json`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::JsonMap;
use crate::auth::{AuthParamError, DISPLAY_NAME_MAX_LENGTH, display_name};
use crate::refusal::{RefusalReason, Refusals, shown_name};

/// The longest part of a name, in UTF-16 code units: as long as a display name.
pub const NAME_PART_MAX_LENGTH: usize = DISPLAY_NAME_MAX_LENGTH;

/// The most credentials a person lists.
pub const MAX_CREDENTIALS: usize = 3;

/// The text is no code of the list it was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a code of this list")]
pub struct UnknownCode;

/// Defines a closed list of codes, stored and sent as the code and shown in the words
/// of the screen's language.
macro_rules! code_list {
    (
        $(#[doc = $doc:literal])*
        $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $code:literal),+ $(,)? }
    ) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl $name {
            /// Every value, in the order of the list.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Every code, in the order of the list.
            pub const CODES: &'static [&'static str] = &[$($code),+];

            /// The value's code.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownCode;

            fn from_str(code: &str) -> Result<Self, UnknownCode> {
                match code {
                    $($code => Ok(Self::$variant),)+
                    _ => Err(UnknownCode),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let code = String::deserialize(deserializer)?;
                code.parse().map_err(|_| {
                    serde::de::Error::unknown_variant(&code, Self::CODES)
                })
            }
        }

        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                let description = [$($doc),*].map(str::trim).join("\n");
                utoipa::openapi::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::Type::String)
                    .enum_values(Some(Self::CODES.iter().copied()))
                    .description(Some(description))
                    .into()
            }
        }

        impl utoipa::ToSchema for $name {
            fn name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }
        }
    };
}

code_list! {
    /// Which name a person is shown by. When it is empty the next one down the list is
    /// taken; the login name is the last.
    NameShown {
        /// The name as they write it, `lastName` and `firstName`.
        Name => "name",
        /// The name in Latin letters, `latinFirstName` and `latinLastName`.
        LatinName => "latin_name",
        /// The name they wrote to be shown by, `customName`.
        CustomName => "custom_name",
        /// The login name.
        Username => "username",
    }
}

impl Default for NameShown {
    /// The first of the list: a new account is shown by its name once it has one.
    fn default() -> Self {
        Self::Name
    }
}

code_list! {
    /// A title written before a person's name.
    Title {
        /// Doctor.
        Dr => "dr",
        /// Professor.
        Prof => "prof",
    }
}

code_list! {
    /// A degree or a professional qualification written after a person's name.
    Credential {
        /// Doctor of Philosophy.
        Phd => "phd",
        /// Doctor of Medicine.
        Md => "md",
        /// Doctor of Dental Surgery.
        Dds => "dds",
        /// Juris Doctor.
        Jd => "jd",
        /// Doctor of Education.
        Edd => "edd",
        /// Master of Business Administration.
        Mba => "mba",
        /// Master of Science.
        Ms => "ms",
        /// Master of Arts.
        Ma => "ma",
        /// Certified Public Accountant.
        Cpa => "cpa",
        /// Chartered Financial Analyst.
        Cfa => "cfa",
        /// Professional Engineer.
        Pe => "pe",
        /// Registered Nurse.
        Rn => "rn",
        /// Attorney at law in Korea (변호사).
        KrAttorney => "kr_attorney",
        /// Patent attorney in Korea (변리사).
        KrPatentAttorney => "kr_patent_attorney",
        /// Certified tax accountant in Korea (세무사).
        KrTaxAccountant => "kr_tax_accountant",
        /// Certified public accountant in Korea (공인회계사).
        KrCpa => "kr_cpa",
        /// Registered architect in Korea (건축사).
        KrArchitect => "kr_architect",
        /// Professional engineer in Korea (기술사).
        KrProfessionalEngineer => "kr_professional_engineer",
        /// Pharmacist in Korea (약사).
        KrPharmacist => "kr_pharmacist",
        /// Doctor of Korean medicine (한의사).
        KrKoreanMedicineDoctor => "kr_korean_medicine_doctor",
    }
}

/// A person's name, every part of which may be empty.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct PersonName {
    /// The given name, in the letters the person writes it in.
    pub first_name: String,
    /// The family name, in the letters the person writes it in.
    pub last_name: String,
    /// The given name in Latin letters.
    pub latin_first_name: String,
    /// The family name in Latin letters.
    pub latin_last_name: String,
    /// The name the person wrote to be shown by.
    pub custom_name: String,
    /// Which name the person is shown by.
    pub name_shown: NameShown,
    /// The title written before the name; `null` for none.
    pub title: Option<Title>,
    /// The degrees and qualifications written after the name, in the person's order: at
    /// most three, each once.
    pub credentials: Vec<Credential>,
}

impl PersonName {
    /// The name the person is shown by: the one they chose, or the next one down
    /// [`NameShown`]'s list when it is empty. Empty when that is the login name, which
    /// whoever shows the person then shows.
    ///
    /// The name as they write it is the family name and the given name: joined, family
    /// name first, when both are written in Hangul, Han or Kana alone ("김영환"), and
    /// the given name, a space and the family name otherwise ("Ana Lima"). The name in
    /// Latin letters is the given name, a space and the family name. A part that is
    /// empty, or white space alone, is left out.
    #[must_use]
    pub fn shown(&self) -> String {
        let start = NameShown::ALL
            .iter()
            .position(|&shown| shown == self.name_shown)
            .unwrap_or_default();
        NameShown::ALL[start..]
            .iter()
            .map(|&shown| match shown {
                NameShown::Name => self.full_name(),
                NameShown::LatinName => self.latin_name(),
                NameShown::CustomName => self.custom_name.trim().to_owned(),
                NameShown::Username => String::new(),
            })
            .find(|name| !name.is_empty())
            .unwrap_or_default()
    }

    /// The name as the person writes it, by the rule of [`PersonName::shown`].
    #[must_use]
    pub fn full_name(&self) -> String {
        let (first, last) = (self.first_name.trim(), self.last_name.trim());
        if is_east_asian(first) && is_east_asian(last) {
            format!("{last}{first}")
        } else {
            joined(first, last)
        }
    }

    /// The name in Latin letters: the given name, a space and the family name.
    #[must_use]
    pub fn latin_name(&self) -> String {
        joined(self.latin_first_name.trim(), self.latin_last_name.trim())
    }

    /// Whether `wanted`, in small letters, is in a part of the name, in the name as the
    /// person writes it or in Latin letters, or in the name they wrote to be shown by:
    /// what finds a person beside their login name.
    #[must_use]
    pub fn contains(&self, wanted: &str) -> bool {
        [
            self.full_name(),
            self.latin_name(),
            // Either order: "Kim Younghwan" finds "Younghwan Kim".
            joined(self.latin_last_name.trim(), self.latin_first_name.trim()),
            joined(self.last_name.trim(), self.first_name.trim()),
            self.custom_name.clone(),
        ]
        .iter()
        .any(|name| name.to_lowercase().contains(wanted))
    }

    /// The credentials as they are stored: their codes, a space between each.
    #[must_use]
    pub fn stored_credentials(&self) -> String {
        let codes: Vec<&str> = self.credentials.iter().map(|c| c.as_str()).collect();
        codes.join(" ")
    }

    /// The credentials from how they are stored; a code this build does not know is
    /// left out.
    #[must_use]
    pub fn credentials_from_stored(stored: &str) -> Vec<Credential> {
        stored
            .split_whitespace()
            .filter_map(|code| code.parse().ok())
            .collect()
    }

    /// Applies what `change` names, leaving the rest as it is.
    pub fn apply(&mut self, change: &PersonNameChange) {
        let PersonNameChange {
            first_name,
            last_name,
            latin_first_name,
            latin_last_name,
            custom_name,
            name_shown,
            title,
            credentials,
        } = change;
        for (part, changed) in [
            (&mut self.first_name, first_name),
            (&mut self.last_name, last_name),
            (&mut self.latin_first_name, latin_first_name),
            (&mut self.latin_last_name, latin_last_name),
            (&mut self.custom_name, custom_name),
        ] {
            if let Some(changed) = changed {
                part.clone_from(changed);
            }
        }
        if let Some(shown) = name_shown {
            self.name_shown = *shown;
        }
        if let Some(title) = title {
            self.title = *title;
        }
        if let Some(credentials) = credentials {
            self.credentials.clone_from(credentials);
        }
    }

    /// The parts of a name from what a provider says of the person when their account
    /// is made: each kept only if it is one a person may give, else left empty. Shown
    /// by the given and family name when both came through, else by the provider's
    /// whole name, so that half a name is not what others see.
    #[must_use]
    pub fn from_provider(
        name: Option<&str>,
        given_name: Option<&str>,
        family_name: Option<&str>,
    ) -> Self {
        let read = |raw: Option<&str>| raw.and_then(|raw| display_name(raw).ok());
        let first_name = read(given_name).unwrap_or_default();
        let last_name = read(family_name).unwrap_or_default();
        let name_shown = if first_name.is_empty() || last_name.is_empty() {
            NameShown::CustomName
        } else {
            NameShown::Name
        };
        Self {
            first_name,
            last_name,
            custom_name: read(name).unwrap_or_default(),
            name_shown,
            ..Self::default()
        }
    }
}

/// Joins a given name and a family name with a space; either alone if the other is
/// empty.
fn joined(first: &str, last: &str) -> String {
    match (first.is_empty(), last.is_empty()) {
        (false, false) => format!("{first} {last}"),
        (false, true) => first.to_owned(),
        _ => last.to_owned(),
    }
}

/// Whether `part` has a letter of Hangul, Han or Kana and no letter of another script.
fn is_east_asian(part: &str) -> bool {
    let mut any = false;
    for c in part.chars() {
        if is_east_asian_char(c) {
            any = true;
        } else if c.is_alphabetic() {
            return false;
        }
    }
    any
}

/// The blocks of Hangul, Han and Kana; the web app's preview has the same
/// (`person-names.ts`).
const EAST_ASIAN: [std::ops::RangeInclusive<char>; 14] = [
    '\u{1100}'..='\u{11FF}',
    '\u{3005}'..='\u{3007}',
    '\u{3040}'..='\u{309F}',
    '\u{30A0}'..='\u{30FF}',
    '\u{3130}'..='\u{318F}',
    '\u{31F0}'..='\u{31FF}',
    '\u{3400}'..='\u{4DBF}',
    '\u{4E00}'..='\u{9FFF}',
    '\u{A960}'..='\u{A97F}',
    '\u{AC00}'..='\u{D7FF}',
    '\u{F900}'..='\u{FAFF}',
    '\u{FF66}'..='\u{FF9F}',
    '\u{FFA0}'..='\u{FFDC}',
    '\u{20000}'..='\u{323AF}',
];

fn is_east_asian_char(c: char) -> bool {
    EAST_ASIAN.iter().any(|range| range.contains(&c))
}

/// What a person changes of their name. A member left out stays as it is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersonNameChange {
    /// The given name as they write it.
    pub first_name: Option<String>,
    /// The family name as they write it.
    pub last_name: Option<String>,
    /// The given name in Latin letters.
    pub latin_first_name: Option<String>,
    /// The family name in Latin letters.
    pub latin_last_name: Option<String>,
    /// The name they wrote to be shown by.
    pub custom_name: Option<String>,
    /// Which name they are shown by.
    pub name_shown: Option<NameShown>,
    /// The title; `Some(None)` for none.
    pub title: Option<Option<Title>>,
    /// The credentials, replacing those there were.
    pub credentials: Option<Vec<Credential>>,
}

impl PersonNameChange {
    /// Reads the change from the members of a request body. A part of the name is a
    /// string or `null` (empty), read as [`display_name`] reads a display name;
    /// `nameShown` a code of [`NameShown`]; `title` a code of [`Title`] or `null`;
    /// `credentials` a list of at most [`MAX_CREDENTIALS`] codes of [`Credential`], each
    /// once, or `null` (none).
    ///
    /// # Errors
    ///
    /// Returns every member that is no such member or is refused, not the first alone.
    pub fn from_members(members: &JsonMap) -> Result<Self, Refusals> {
        let mut change = Self::default();
        let mut refused = Refusals::new();
        for (name, value) in members {
            let part = match name.as_str() {
                "firstName" => Some(("firstName", &mut change.first_name)),
                "lastName" => Some(("lastName", &mut change.last_name)),
                "latinFirstName" => Some(("latinFirstName", &mut change.latin_first_name)),
                "latinLastName" => Some(("latinLastName", &mut change.latin_last_name)),
                "customName" => Some(("customName", &mut change.custom_name)),
                _ => None,
            };
            if let Some((field, part)) = part {
                match name_part(field, value) {
                    Ok(read) => *part = Some(read),
                    Err(error) => refused.push(error.into()),
                }
                continue;
            }
            match name.as_str() {
                // Always one of the list: `null` leaves the choice as it is.
                "nameShown" if value.is_null() => {}
                "nameShown" => change.name_shown = read_code(&mut refused, name, value),
                "title" if value.is_null() => change.title = Some(None),
                "title" => change.title = read_code(&mut refused, name, value).map(Some),
                "credentials" => change.credentials = read_credentials(&mut refused, value),
                other => refused.refuse(
                    other,
                    RefusalReason::Unknown,
                    format!("there is no member {:?}", shown_name(other)),
                ),
            }
        }
        refused.into_result().map(|()| change)
    }

    /// Whether the change changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Reads the part of a name in the member `field`: a string as [`display_name`] reads
/// one, or `null` for none.
fn name_part(field: &'static str, value: &serde_json::Value) -> Result<String, AuthParamError> {
    let raw = match value {
        serde_json::Value::Null => return Ok(String::new()),
        serde_json::Value::String(raw) => raw,
        _ => {
            return Err(AuthParamError::new(
                field,
                RefusalReason::Invalid,
                format!("{field} must be a string or null"),
            ));
        }
    };
    display_name(raw).map_err(|error| {
        let message = match error.reason() {
            RefusalReason::TooLong => {
                format!("{field} has at most {NAME_PART_MAX_LENGTH} characters")
            }
            _ => format!(
                "{field} has no control characters, and none that show nothing or turn the \
                 text around"
            ),
        };
        AuthParamError::new(field, error.reason(), message)
    })
}

/// Reads a code of a list from the member `member`, or refuses it.
fn read_code<T: FromStr>(
    refused: &mut Refusals,
    member: &str,
    value: &serde_json::Value,
) -> Option<T> {
    let read = value.as_str().and_then(|code| code.parse().ok());
    if read.is_none() {
        refused.refuse(
            member,
            RefusalReason::Invalid,
            format!("{member} must be a code of its list"),
        );
    }
    read
}

/// Reads the list of credentials, or refuses what is wrong with it.
fn read_credentials(refused: &mut Refusals, value: &serde_json::Value) -> Option<Vec<Credential>> {
    let items = match value {
        serde_json::Value::Null => return Some(Vec::new()),
        serde_json::Value::Array(items) => items,
        _ => {
            refused.refuse(
                "credentials",
                RefusalReason::Invalid,
                "credentials must be a list of codes or null",
            );
            return None;
        }
    };
    if items.len() > MAX_CREDENTIALS {
        refused.refuse(
            "credentials",
            RefusalReason::TooLong,
            format!("credentials has at most {MAX_CREDENTIALS} codes"),
        );
        return None;
    }
    let mut read = Vec::with_capacity(items.len());
    let mut wrong = false;
    for (index, item) in items.iter().enumerate() {
        let at = format!("/credentials/{index}");
        match item.as_str().map(str::parse::<Credential>) {
            Some(Ok(code)) if read.contains(&code) => {
                refused.refuse_at(at, RefusalReason::Invalid, "a credential is listed once");
                wrong = true;
            }
            Some(Ok(code)) => read.push(code),
            _ => {
                refused.refuse_at(
                    at,
                    RefusalReason::Invalid,
                    "a credential is a code of its list",
                );
                wrong = true;
            }
        }
    }
    (!wrong).then_some(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(first: &str, last: &str, latin: (&str, &str), custom: &str) -> PersonName {
        PersonName {
            first_name: first.to_owned(),
            last_name: last.to_owned(),
            latin_first_name: latin.0.to_owned(),
            latin_last_name: latin.1.to_owned(),
            custom_name: custom.to_owned(),
            ..PersonName::default()
        }
    }

    /// Each choice is taken when it is there, and the next one down when it is not;
    /// the login name, last, is shown as no name.
    #[test]
    fn the_choice_goes_down_the_list_when_it_is_empty() {
        let full = named("영환", "김", ("Younghwan", "Kim"), "YH");
        let cases = [
            (NameShown::Name, &full, "김영환"),
            (NameShown::LatinName, &full, "Younghwan Kim"),
            (NameShown::CustomName, &full, "YH"),
            (NameShown::Username, &full, ""),
            (
                NameShown::Name,
                &named("", "", ("Younghwan", "Kim"), "YH"),
                "Younghwan Kim",
            ),
            (NameShown::Name, &named("", " ", ("", ""), "YH"), "YH"),
            (NameShown::Name, &named("", "", ("", ""), ""), ""),
            (
                NameShown::LatinName,
                &named("영환", "김", ("", ""), "YH"),
                "YH",
            ),
            (NameShown::LatinName, &named("영환", "김", ("", ""), ""), ""),
            (
                NameShown::CustomName,
                &named("영환", "김", ("Y", "K"), ""),
                "",
            ),
        ];
        for (shown, name, want) in cases {
            let name = PersonName {
                name_shown: shown,
                ..name.clone()
            };
            assert_eq!(name.shown(), want, "{name:?}");
        }
    }

    /// East Asian names are joined family name first; any other letter makes it the
    /// given name, a space and the family name.
    #[test]
    fn the_script_decides_the_order_of_the_parts() {
        let cases = [
            ("영환", "김", "김영환"),
            ("太郎", "山田", "山田太郎"),
            ("たろう", "やまだ", "やまだたろう"),
            ("Ana", "Lima", "Ana Lima"),
            ("Younghwan", "김", "Younghwan 김"),
            ("영환", "Kim", "영환 Kim"),
            ("영환", "", "영환"),
            ("", "김", "김"),
            ("𠀋", "김", "김𠀋"),
        ];
        for (first, last, want) in cases {
            assert_eq!(named(first, last, ("", ""), "").full_name(), want);
        }
    }

    #[test]
    fn a_change_is_read_from_its_members_and_refused_whole() {
        let members: JsonMap = serde_json::from_value(serde_json::json!({
            "firstName": " 영환 ", "lastName": null, "nameShown": "latin_name",
            "title": "dr", "credentials": ["phd", "kr_attorney"],
        }))
        .expect("an object");
        let change = PersonNameChange::from_members(&members).expect("taken");
        assert_eq!(change.first_name.as_deref(), Some("영환"));
        assert_eq!(change.last_name.as_deref(), Some(""));
        assert_eq!(change.name_shown, Some(NameShown::LatinName));
        assert_eq!(change.title, Some(Some(Title::Dr)));
        assert_eq!(
            change.credentials,
            Some(vec![Credential::Phd, Credential::KrAttorney])
        );

        let members: JsonMap = serde_json::from_value(serde_json::json!({
            "title": "sir", "credentials": ["phd", "phd", "esq"], "nameShown": 3,
            "displayName": "x", "latinLastName": "a\u{202E}b",
        }))
        .expect("an object");
        let refused = PersonNameChange::from_members(&members).expect_err("refused");
        let pointers: Vec<&str> = refused.iter().map(|r| r.pointer.as_str()).collect();
        assert_eq!(
            pointers,
            [
                "/credentials/1",
                "/credentials/2",
                "/displayName",
                "/latinLastName",
                "/nameShown",
                "/title"
            ]
        );
    }

    #[test]
    fn more_than_three_credentials_are_refused() {
        let members: JsonMap = serde_json::from_value(serde_json::json!({
            "credentials": ["phd", "md", "mba", "ms"],
        }))
        .expect("an object");
        let refused = PersonNameChange::from_members(&members).expect_err("refused");
        let reasons: Vec<RefusalReason> = refused.iter().map(|r| r.reason).collect();
        assert_eq!(reasons, [RefusalReason::TooLong]);
    }

    #[test]
    fn credentials_are_stored_by_their_codes_in_order() {
        let name = PersonName {
            credentials: vec![Credential::Mba, Credential::Phd],
            ..PersonName::default()
        };
        assert_eq!(name.stored_credentials(), "mba phd");
        assert_eq!(
            PersonName::credentials_from_stored("mba phd later"),
            name.credentials
        );
    }

    #[test]
    fn a_person_is_found_by_every_part_of_their_name() {
        let name = named("영환", "김", ("Younghwan", "Kim"), "YH");
        for wanted in [
            "김영환",
            "영환",
            "younghwan kim",
            "kim younghwan",
            "yh",
            "김 영환",
        ] {
            assert!(name.contains(wanted), "{wanted}");
        }
        assert!(!name.contains("lee"));
    }

    #[test]
    fn a_null_choice_leaves_it_and_a_filler_is_no_name() {
        let members: JsonMap =
            serde_json::from_value(serde_json::json!({ "nameShown": null })).expect("an object");
        let change = PersonNameChange::from_members(&members).expect("taken");
        assert!(change.is_empty(), "null leaves the choice as it is");

        // The Hangul fillers and the blank Braille pattern show nothing, the grapheme
        // joiner, the Mongolian vowel separator and the Arabic letter mark neither, or
        // turn the text: refused like U+200B.
        for filler in [
            '\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}', '\u{2800}', '\u{034F}', '\u{180E}',
            '\u{061C}',
        ] {
            let members: JsonMap =
                serde_json::from_value(serde_json::json!({ "firstName": filler.to_string() }))
                    .expect("an object");
            let refused = PersonNameChange::from_members(&members).expect_err("refused");
            let pointers: Vec<&str> = refused.iter().map(|r| r.pointer.as_str()).collect();
            assert_eq!(pointers, ["/firstName"], "U+{:04X}", u32::from(filler));
        }
    }

    #[test]
    fn a_provider_shows_its_whole_name_unless_both_parts_came() {
        let both = PersonName::from_provider(Some("Ana Lima"), Some("Ana"), Some("Lima"));
        assert_eq!(
            (both.name_shown, both.shown().as_str()),
            (NameShown::Name, "Ana Lima")
        );
        let half = PersonName::from_provider(Some("Ana Lima"), Some("Ana"), None);
        assert_eq!(
            (
                half.name_shown,
                half.first_name.as_str(),
                half.shown().as_str()
            ),
            (NameShown::CustomName, "Ana", "Ana Lima")
        );
        let refused = PersonName::from_provider(None, Some("Ana"), Some("Li\u{202E}ma"));
        assert_eq!(refused.name_shown, NameShown::CustomName);
        assert_eq!(refused.shown(), "", "nothing whole to show: the login name");
    }
}
