//! What a user has set about how things are shown to them, and the small bag of state a
//! client keeps with the account.
//!
//! The settings are a closed set: the server knows every one, its values and its
//! default, and refuses anything else; of a language it knows only the shape of a tag.
//! They belong to the account and not to a device, so that the same person sees the same
//! times on every device.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::JsonMap;
use crate::refusal::{RefusalReason, Refusals, shown_name};

/// The text is no value of the setting it was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a value of this setting")]
pub struct ParseSettingError;

/// Defines a setting whose values are a closed set of words.
macro_rules! word_setting {
    (
        $(#[doc = $doc:literal])*
        $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $word:literal),+ $(,)? }
    ) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                #[serde(rename = $word)]
                $variant,
            )+
        }

        impl $name {
            /// Every value, as it is written.
            pub const WORDS: &'static [&'static str] = &[$($word),+];

            /// The value as it is written.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word,)+
                }
            }
        }

        // Written out and not derived: the derive reads the name of a variant where it
        // cannot read the `rename` this macro writes, and would list `Auto` where the
        // value is `auto`.
        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                let description = [$($doc),*].map(str::trim).join("\n");
                utoipa::openapi::ObjectBuilder::new()
                    .schema_type(utoipa::openapi::schema::Type::String)
                    .enum_values(Some(Self::WORDS.iter().copied()))
                    .description(Some(description))
                    .into()
            }
        }

        impl utoipa::ToSchema for $name {
            fn name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::Borrowed(stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = ParseSettingError;

            fn from_str(word: &str) -> Result<Self, ParseSettingError> {
                match word {
                    $($word => Ok(Self::$variant),)+
                    _ => Err(ParseSettingError),
                }
            }
        }
    };
}

word_setting! {
    /// The day a week starts on.
    WeekStart {
        /// As the device's locale has it.
        #[default]
        Auto => "auto",
        /// Weeks start on Sunday.
        Sunday => "sunday",
        /// Weeks start on Monday.
        Monday => "monday",
        /// Weeks start on Saturday.
        Saturday => "saturday",
    }
}

word_setting! {
    /// How the hours of a day are written. The names are those of the browsers' `Intl`.
    HourCycle {
        /// As the device's locale has it.
        #[default]
        Auto => "auto",
        /// 1 to 12, with a.m. and p.m.
        H12 => "h12",
        /// 0 to 23.
        H23 => "h23",
    }
}

word_setting! {
    /// The colors of the interface.
    Theme {
        /// As the device has it.
        #[default]
        Auto => "auto",
        /// Light colors, whatever the device has.
        Light => "light",
        /// Dark colors, whatever the device has.
        Dark => "dark",
    }
}

word_setting! {
    /// Where a card opens.
    CardOpen {
        /// In a dialog in the middle of the board.
        #[default]
        Center => "center",
        /// In a panel at the side of the board.
        Side => "side",
        /// As a page of its own.
        Full => "full",
    }
}

word_setting! {
    /// How much room the interface gives each thing.
    Density {
        /// As fits the device.
        #[default]
        Auto => "auto",
        /// Less room: more on the screen.
        Compact => "compact",
        /// More room.
        Comfortable => "comfortable",
    }
}

/// The time zone times are shown in: that of the device, or one by its IANA name.
///
/// A name is one the time zone database compiled into this build knows, spelled as the
/// database spells it (names are case-sensitive). Names the database keeps for
/// compatibility (`Asia/Calcutta`) are names too, and are kept as given. `Factory`, the
/// database's zone for "no time zone was set", is none.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct TimeZone(Option<String>);

impl TimeZone {
    /// The word for "the time zone of the device".
    pub const AUTO: &'static str = "auto";
    /// Maximum length in bytes of a time zone identifier accepted by the server.
    pub const MAX_LENGTH: usize = 64;

    /// The setting as it is written: `auto` or the name of the time zone.
    pub fn as_str(&self) -> &str {
        self.0.as_deref().unwrap_or(Self::AUTO)
    }

    /// The name of the time zone, or `None` where it is left to the device.
    pub(crate) fn name(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl FromStr for TimeZone {
    type Err = ParseSettingError;

    fn from_str(text: &str) -> Result<Self, ParseSettingError> {
        if text == Self::AUTO {
            Ok(Self(None))
        } else if is_time_zone_name(text) {
            Ok(Self(Some(text.to_owned())))
        } else {
            Err(ParseSettingError)
        }
    }
}

/// The database's zone of an installation whose time zone was never set: no place's.
const UNSET_ZONE: &str = "Factory";

/// Whether `name` is a time zone of the compiled-in database, spelled as it spells it.
/// The database is the bundled one by name, never the host's, whatever features of
/// `jiff` another crate turns on. It finds names whatever their case and names an
/// unknown zone as none, so a name is one only when the zone found has that very name.
fn is_time_zone_name(name: &str) -> bool {
    name.len() <= TimeZone::MAX_LENGTH
        && name != UNSET_ZONE
        && jiff::tz::TimeZoneDatabase::bundled()
            .get(name)
            .is_ok_and(|zone| zone.iana_name() == Some(name))
}

/// The language of the interface: that of the browser, or one by its language tag.
///
/// The server keeps no list of languages; it checks that a tag has the shape the
/// browsers' `Intl` reads (a Unicode BCP 47 locale identifier, `-` between subtags):
/// a language of 2–3 or 5–8 letters, then an optional script of 4 letters, an optional
/// region of 2 letters or 3 digits, variants of 5–8 letters and digits or a digit and 3
/// (none twice), extensions (a letter or digit other than `x`, then subtags of 2–8) and
/// private use (`x`, then subtags of 1–8) last. At most [`Language::MAX_LENGTH`]
/// characters. A tag is kept as given, case and all (`en-us` stays `en-us`; tags do not
/// differ by case).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Language(Option<String>);

impl Language {
    /// The word for "the language of the browser".
    pub const AUTO: &'static str = "auto";
    /// The longest tag the server takes: room for a tag with a `-u-` extension, such as
    /// what `Intl.Locale` writes.
    pub const MAX_LENGTH: usize = 64;

    /// The setting as it is written: `auto` or the tag.
    pub fn as_str(&self) -> &str {
        self.0.as_deref().unwrap_or(Self::AUTO)
    }
}

/// Whether `tag` has the shape of a Unicode BCP 47 locale identifier.
fn is_locale_tag(tag: &str) -> bool {
    let letters = |s: &str, lengths: std::ops::RangeInclusive<usize>| {
        lengths.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic())
    };
    let alphanumerics = |s: &str, lengths: std::ops::RangeInclusive<usize>| {
        lengths.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    let mut subtags = tag.split('-').peekable();
    let language = subtags.next().unwrap_or_default();
    if !(letters(language, 2..=3) || letters(language, 5..=8)) {
        return false;
    }
    let _script = subtags.next_if(|s| letters(s, 4..=4));
    let is_region =
        |s: &&str| letters(s, 2..=2) || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()));
    let _region = subtags.next_if(is_region);
    let is_variant = |s: &&str| {
        alphanumerics(s, 5..=8)
            || (s.len() == 4 && s.as_bytes()[0].is_ascii_digit() && alphanumerics(s, 4..=4))
    };
    let mut variants = BTreeSet::new();
    while let Some(variant) = subtags.next_if(is_variant) {
        if !variants.insert(variant.to_ascii_lowercase()) {
            return false;
        }
    }
    let mut singletons = BTreeSet::new();
    while let Some(singleton) = subtags.next() {
        if !alphanumerics(singleton, 1..=1) || !singletons.insert(singleton.to_ascii_lowercase()) {
            return false;
        }
        let private = singleton.eq_ignore_ascii_case("x");
        let lengths = if private { 1..=8 } else { 2..=8 };
        let mut any = false;
        while subtags
            .next_if(|s| alphanumerics(s, lengths.clone()))
            .is_some()
        {
            any = true;
        }
        if !any {
            return false;
        }
        // Private use is last: whatever follows it is part of it.
        if private {
            return subtags.next().is_none();
        }
    }
    true
}

impl FromStr for Language {
    type Err = ParseSettingError;

    fn from_str(text: &str) -> Result<Self, ParseSettingError> {
        if text == Self::AUTO {
            Ok(Self(None))
        } else if text.len() <= Self::MAX_LENGTH && is_locale_tag(text) {
            Ok(Self(Some(text.to_owned())))
        } else {
            Err(ParseSettingError)
        }
    }
}

impl fmt::Debug for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Language({})", self.as_str())
    }
}

impl Serialize for Language {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Debug for TimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TimeZone({})", self.as_str())
    }
}

impl Serialize for TimeZone {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The settings of a user. Every member has a value: what was never set, or was reset,
/// is at its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each switch is a separate setting of the wire format, on or off"
)]
pub struct UserSettings {
    /// The time zone times are shown in.
    #[schema(value_type = String)]
    pub time_zone: TimeZone,
    /// The day a week starts on.
    pub week_start: WeekStart,
    /// How the hours of a day are written.
    pub hour_cycle: HourCycle,
    /// The colors of the interface.
    pub theme: Theme,
    /// Where a card opens.
    pub card_open: CardOpen,
    /// How much room the interface gives each thing.
    pub density: Density,
    /// Whether a device that can do so answers touches with a vibration.
    pub haptics: bool,
    /// The language of the interface: `auto` for the browser's, or a language tag such
    /// as `ko` or `en-US`, in the shape the browsers' `Intl` reads (Unicode BCP 47), at
    /// most 64 characters. The server keeps no list of languages, and answers a tag as
    /// it was given, case and all: compare tags without regard to case.
    #[schema(value_type = String)]
    pub language: Language,
    /// Whether the user is told when somebody mentions them (`mention`).
    pub notify_mentions: bool,
    /// Whether the user is told when somebody gives them a card (`assignment`).
    pub notify_assignments: bool,
    /// Whether the user is told of the new comments on what they watch (`comment`).
    pub notify_comments: bool,
    /// Whether the user is told when an incomplete assigned card is due soon (`due_soon`).
    pub notify_due: bool,
}

impl UserSettings {
    /// Whether the user is told of what is of `kind`. What is off is not made at
    /// all: switched on again, nothing of the time between comes.
    #[must_use]
    pub const fn notifies(&self, kind: crate::NotificationKind) -> bool {
        match kind {
            crate::NotificationKind::Mention => self.notify_mentions,
            crate::NotificationKind::Assignment => self.notify_assignments,
            crate::NotificationKind::Comment => self.notify_comments,
            crate::NotificationKind::DueSoon => self.notify_due,
            crate::NotificationKind::ReviewDue
            | crate::NotificationKind::MeetingUpcoming
            | crate::NotificationKind::AddedToContainer => true,
        }
    }
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            time_zone: TimeZone::default(),
            week_start: WeekStart::default(),
            hour_cycle: HourCycle::default(),
            theme: Theme::default(),
            card_open: CardOpen::default(),
            density: Density::default(),
            haptics: true,
            language: Language::default(),
            notify_mentions: true,
            notify_assignments: true,
            notify_comments: true,
            notify_due: true,
        }
    }
}

/// One of the settings, by its name on the wire and in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SettingName {
    /// `timeZone`
    TimeZone,
    /// `weekStart`
    WeekStart,
    /// `hourCycle`
    HourCycle,
    /// `theme`
    Theme,
    /// `cardOpen`
    CardOpen,
    /// `density`
    Density,
    /// `haptics`
    Haptics,
    /// `language`
    Language,
    /// `notifyMentions`
    NotifyMentions,
    /// `notifyAssignments`
    NotifyAssignments,
    /// `notifyComments`
    NotifyComments,
    /// `notifyDue`
    NotifyDue,
}

impl SettingName {
    /// Every setting there is.
    pub const ALL: [Self; 12] = [
        Self::TimeZone,
        Self::WeekStart,
        Self::HourCycle,
        Self::Theme,
        Self::CardOpen,
        Self::Density,
        Self::Haptics,
        Self::Language,
        Self::NotifyMentions,
        Self::NotifyAssignments,
        Self::NotifyComments,
        Self::NotifyDue,
    ];

    /// The name as it is written.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TimeZone => "timeZone",
            Self::WeekStart => "weekStart",
            Self::HourCycle => "hourCycle",
            Self::Theme => "theme",
            Self::CardOpen => "cardOpen",
            Self::Density => "density",
            Self::Haptics => "haptics",
            Self::Language => "language",
            Self::NotifyMentions => "notifyMentions",
            Self::NotifyAssignments => "notifyAssignments",
            Self::NotifyComments => "notifyComments",
            Self::NotifyDue => "notifyDue",
        }
    }

    /// Whether the setting is on or off, `true` or `false`.
    const fn is_flag(self) -> bool {
        matches!(
            self,
            Self::Haptics
                | Self::NotifyMentions
                | Self::NotifyAssignments
                | Self::NotifyComments
                | Self::NotifyDue
        )
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == name)
    }

    /// What a value of this setting may be, for the message of a refusal.
    fn expectation(self) -> String {
        let words = |words: &[&str]| {
            let quoted: Vec<String> = words.iter().map(|word| format!("\"{word}\"")).collect();
            format!("one of {}", quoted.join(", "))
        };
        match self {
            Self::TimeZone => "\"auto\" or the name of an IANA time zone".to_owned(),
            Self::WeekStart => words(WeekStart::WORDS),
            Self::HourCycle => words(HourCycle::WORDS),
            Self::Theme => words(Theme::WORDS),
            Self::CardOpen => words(CardOpen::WORDS),
            Self::Density => words(Density::WORDS),
            Self::Haptics
            | Self::NotifyMentions
            | Self::NotifyAssignments
            | Self::NotifyComments
            | Self::NotifyDue => "true or false".to_owned(),
            Self::Language => "\"auto\" or a language tag such as \"ko\" or \"en-US\"".to_owned(),
        }
    }
}

impl UserSettings {
    /// The settings as stored values say them: `(name, value)` pairs as
    /// [`UserSettingsPatch::changes`] produced them. A name this build does not know and
    /// a value it would not accept are passed over, and the setting is at its default: a
    /// value written by another build must not take the settings down.
    pub fn from_stored<'a>(stored: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut settings = Self::default();
        for (name, value) in stored {
            if let Some(name) = SettingName::from_name(name) {
                // A stored value that does not parse leaves the default in place.
                let _unreadable: Result<(), ParseSettingError> = settings.set(name, value);
            }
        }
        settings
    }

    /// Sets one setting from its stored form.
    fn set(&mut self, name: SettingName, value: &str) -> Result<(), ParseSettingError> {
        match name {
            SettingName::TimeZone => self.time_zone = value.parse()?,
            SettingName::WeekStart => self.week_start = value.parse()?,
            SettingName::HourCycle => self.hour_cycle = value.parse()?,
            SettingName::Theme => self.theme = value.parse()?,
            SettingName::CardOpen => self.card_open = value.parse()?,
            SettingName::Density => self.density = value.parse()?,
            SettingName::Haptics => self.haptics = parse_flag(value)?,
            SettingName::NotifyMentions => self.notify_mentions = parse_flag(value)?,
            SettingName::NotifyAssignments => self.notify_assignments = parse_flag(value)?,
            SettingName::NotifyComments => self.notify_comments = parse_flag(value)?,
            SettingName::NotifyDue => self.notify_due = parse_flag(value)?,
            SettingName::Language => self.language = value.parse()?,
        }
        Ok(())
    }
}

/// A setting that is on or off, as it is stored.
fn parse_flag(value: &str) -> Result<bool, ParseSettingError> {
    value.parse().map_err(|_| ParseSettingError)
}

/// A change to a user's settings: some set to a value, some put back to their default.
/// The settings it does not name stay as they are.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserSettingsPatch {
    set: BTreeMap<SettingName, String>,
    reset: BTreeSet<SettingName>,
}

/// Why a change to the settings was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The request names a setting there is not.
    #[error("there is no setting {name:?}")]
    UnknownSetting {
        /// The name the request used.
        name: String,
    },
    /// The value is none the setting can have.
    #[error("{name} must be {expected}")]
    InvalidValue {
        /// The setting, by its name on the wire.
        name: &'static str,
        /// What a value of the setting may be.
        expected: String,
    },
}

impl SettingsError {
    /// The member of the request that is wrong.
    pub fn field(&self) -> &str {
        match self {
            Self::UnknownSetting { name } => name,
            Self::InvalidValue { name, .. } => name,
        }
    }

    /// Why a client is told the member was refused.
    fn reason(&self) -> RefusalReason {
        match self {
            Self::UnknownSetting { .. } => RefusalReason::Unknown,
            Self::InvalidValue { .. } => RefusalReason::Invalid,
        }
    }
}

/// What a member of a change to the settings asks for.
enum SettingChange {
    /// The setting gets this value, as it is stored.
    Set(SettingName, String),
    /// The setting goes back to its default.
    Reset(SettingName),
}

impl SettingChange {
    /// Reads one member of a request.
    fn read(name: &str, value: &serde_json::Value) -> Result<Self, SettingsError> {
        let setting =
            SettingName::from_name(name).ok_or_else(|| SettingsError::UnknownSetting {
                name: shown_name(name),
            })?;
        let invalid = || SettingsError::InvalidValue {
            name: setting.as_str(),
            expected: setting.expectation(),
        };
        let stored = match (setting, value) {
            (_, serde_json::Value::Null) => return Ok(Self::Reset(setting)),
            (setting, serde_json::Value::Bool(flag)) if setting.is_flag() => flag.to_string(),
            (setting, _) if setting.is_flag() => return Err(invalid()),
            (_, serde_json::Value::String(text)) => text.clone(),
            _ => return Err(invalid()),
        };
        // Whatever is stored must read back: checked with the reader itself.
        UserSettings::default()
            .set(setting, &stored)
            .map_err(|ParseSettingError| invalid())?;
        Ok(Self::Set(setting, stored))
    }
}

impl UserSettingsPatch {
    /// Reads a change from the members of a request: a value sets the setting, `null`
    /// puts it back to its default. Every member must be a setting, and every value one
    /// the setting can have.
    ///
    /// # Errors
    ///
    /// Returns every member, in the order of the names, that is no setting (the reason
    /// `unknown`) or whose value the setting cannot have (`invalid`), each with the
    /// words of its [`SettingsError`].
    pub fn from_members(members: &JsonMap) -> Result<Self, Refusals> {
        let mut patch = Self::default();
        let mut refused = Refusals::new();
        for (name, value) in members {
            match SettingChange::read(name, value) {
                Ok(SettingChange::Set(setting, stored)) => {
                    patch.set.insert(setting, stored);
                }
                Ok(SettingChange::Reset(setting)) => {
                    patch.reset.insert(setting);
                }
                Err(error) => refused.refuse(name, error.reason(), error.to_string()),
            }
        }
        refused.into_result().map(|()| patch)
    }

    /// Whether the change changes nothing.
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.reset.is_empty()
    }

    /// The settings to store, as `(name, value)`, and the settings whose stored value is
    /// to be put back to its default, each in the order the settings are listed in.
    pub fn changes(&self) -> (Vec<(SettingName, &str)>, Vec<SettingName>) {
        let set = self
            .set
            .iter()
            .map(|(&name, value)| (name, value.as_str()))
            .collect();
        let reset = self.reset.iter().copied().collect();
        (set, reset)
    }
}

/// The most entries the bag of client state of one user holds.
pub const CLIENT_STATE_MAX_ENTRIES: usize = 100;
/// The longest name of an entry of the bag of client state, in characters.
pub const CLIENT_STATE_MAX_NAME_LENGTH: usize = 64;
/// The longest value of an entry of the bag of client state, in bytes.
pub const CLIENT_STATE_MAX_VALUE_BYTES: usize = 4 * 1024;

/// Why an entry of the bag of client state was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientStateError {
    /// The name is empty, too long, or has a character that is not a letter of the
    /// English alphabet, a digit, `_`, `.` or `-`.
    #[error(
        "the name of an entry is 1 to {CLIENT_STATE_MAX_NAME_LENGTH} of the characters \
         A-Z, a-z, 0-9, '_', '.' and '-'"
    )]
    InvalidName,
    /// The value is too long.
    #[error("the value of an entry is at most {CLIENT_STATE_MAX_VALUE_BYTES} bytes")]
    ValueTooLong,
}

/// Checks the name of an entry of the bag of client state.
///
/// # Errors
///
/// Returns [`ClientStateError::InvalidName`] if the name is none.
pub fn check_client_state_name(name: &str) -> Result<(), ClientStateError> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
    if name.is_empty() || name.len() > CLIENT_STATE_MAX_NAME_LENGTH || !name.chars().all(allowed) {
        Err(ClientStateError::InvalidName)
    } else {
        Ok(())
    }
}

/// Checks the value of an entry of the bag of client state.
///
/// # Errors
///
/// Returns [`ClientStateError::ValueTooLong`] if the value is too long.
pub fn check_client_state_value(value: &str) -> Result<(), ClientStateError> {
    if value.len() > CLIENT_STATE_MAX_VALUE_BYTES {
        Err(ClientStateError::ValueTooLong)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
