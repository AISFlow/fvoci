//! The languages the server writes its own text in: its mail and the built-in templates.

/// A language the server's own text is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Language {
    English,
    Korean,
}

impl Language {
    /// Every language, English first.
    pub(crate) const ALL: [Self; 2] = [Self::English, Self::Korean];

    /// The language that `tag` names (`ko`, `ko-KR`, `en-GB`), or `None` for one the
    /// server writes nothing in (`ja`) and for `auto`.
    pub(crate) fn written_in(tag: &str) -> Option<Self> {
        let language = tag.split('-').next().unwrap_or_default();
        if language.eq_ignore_ascii_case("ko") {
            Some(Self::Korean)
        } else if language.eq_ignore_ascii_case("en") {
            Some(Self::English)
        } else {
            None
        }
    }

    /// The language tag: what the HTML and `Content-Language` of a mail give, and what
    /// a built-in template says it is written in.
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Korean => "ko",
        }
    }

    /// The first of `choices`, most wanted first, that the server writes in, or
    /// English. For mail: the recipient's setting, then the sender's setting (for
    /// invitations), then the language of the screen the request came from.
    pub(crate) fn first_of(choices: impl IntoIterator<Item = Option<Self>>) -> Self {
        choices
            .into_iter()
            .flatten()
            .next()
            .unwrap_or(Self::English)
    }
}
