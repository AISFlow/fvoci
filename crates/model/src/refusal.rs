//! What a check of a request refuses: every member that is wrong, not the first alone.

use std::fmt;

use crate::problem::{json_pointer, pointer_token};

/// Defines [`RefusalReason`] with the value of each variant, and the list of every
/// variant from the same lines: a cause cannot be there without being listed.
macro_rules! refusal_reasons {
    ($($(#[$doc:meta])* $variant:ident => $value:literal;)+) => {
        /// Why a member of a request was refused, as far as a check tells causes apart.
        /// A client chooses its words for the member by it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum RefusalReason {
            $($(#[$doc])* $variant,)+
        }

        impl RefusalReason {
            /// Every cause, for the contract.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The value of the `reason` of the entry of `errors`.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }
    };
}

refusal_reasons! {
    /// The member is missing or blank, and the request cannot do without it.
    Required => "required";
    /// The value is shorter than it may be.
    TooShort => "too_short";
    /// The value is longer than it may be.
    TooLong => "too_long";
    /// The value has not the form it must have: an email address that is none.
    InvalidFormat => "invalid_format";
    /// The value holds a character it may not.
    InvalidCharacters => "invalid_characters";
    /// The request has no such member.
    Unknown => "unknown";
    /// The value is one the member cannot have, for a cause no finer than that.
    Invalid => "invalid";
}

/// How much of a name of the client's own the words of a refusal show.
pub const NAME_SHOWN_CHARS: usize = 64;

/// A name of the client's own as the words of a refusal show it: at most
/// [`NAME_SHOWN_CHARS`] characters of it, since it may be as long as a request.
#[must_use]
pub fn shown_name(name: &str) -> String {
    match name.char_indices().nth(NAME_SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &name[..end]),
        None => name.to_owned(),
    }
}

/// One member of a request body that a check refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Where the member is in the body: a JSON Pointer (RFC 6901).
    pub pointer: String,
    /// Why it was refused.
    pub reason: RefusalReason,
    /// What is wrong with it, in English; sent to the client as it is.
    pub detail: String,
}

/// What a check of a request refused: every member that is wrong, in the order the
/// check looks at them.
///
/// A check gathers into an empty one and ends with [`Refusals::into_result`], which is
/// an error only if something was refused.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Refusals(Vec<Refusal>);

impl Refusals {
    /// Nothing refused yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuses the member `member` of the object the body is.
    pub fn refuse(&mut self, member: &str, reason: RefusalReason, detail: impl Into<String>) {
        self.refuse_at(json_pointer(member), reason, detail);
    }

    /// Refuses the member `member` of the object that is the member `object` of the
    /// body: an entry of a map the request carries.
    pub fn refuse_within(
        &mut self,
        object: &str,
        member: &str,
        reason: RefusalReason,
        detail: impl Into<String>,
    ) {
        let pointer = format!("{}/{}", json_pointer(object), pointer_token(member));
        self.refuse_at(pointer, reason, detail);
    }

    /// Refuses what is at `pointer`, a JSON Pointer into the body.
    pub fn refuse_at(
        &mut self,
        pointer: impl Into<String>,
        reason: RefusalReason,
        detail: impl Into<String>,
    ) {
        self.0.push(Refusal {
            pointer: pointer.into(),
            reason,
            detail: detail.into(),
        });
    }

    /// Adds a refusal that another check made.
    pub fn push(&mut self, refusal: Refusal) {
        self.0.push(refusal);
    }

    /// Whether nothing was refused.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many members were refused.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The refusals, in the order they were made.
    pub fn iter(&self) -> std::slice::Iter<'_, Refusal> {
        self.0.iter()
    }

    /// The end of a check: fine if nothing was refused.
    ///
    /// # Errors
    ///
    /// Returns the refusals if there is one.
    pub fn into_result(self) -> Result<(), Self> {
        if self.0.is_empty() { Ok(()) } else { Err(self) }
    }
}

impl IntoIterator for Refusals {
    type Item = Refusal;
    type IntoIter = std::vec::IntoIter<Refusal>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'r> IntoIterator for &'r Refusals {
    type Item = &'r Refusal;
    type IntoIter = std::slice::Iter<'r, Refusal>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<Refusal> for Refusals {
    fn from(refusal: Refusal) -> Self {
        Self(vec![refusal])
    }
}

impl fmt::Display for Refusals {
    /// The detail of the one refusal, or how many there are: each has its own detail.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_slice() {
            [] => formatter.write_str("nothing of the request was refused"),
            [one] => formatter.write_str(&one.detail),
            many => write!(
                formatter,
                "{} members of the request were refused",
                many.len()
            ),
        }
    }
}

impl std::error::Error for Refusals {}

#[cfg(test)]
mod tests {
    use super::{RefusalReason, Refusals};

    #[test]
    fn a_check_ends_fine_only_if_nothing_was_refused() {
        assert_eq!(Refusals::new().into_result(), Ok(()));

        let mut refusals = Refusals::new();
        refusals.refuse("username", RefusalReason::Required, "username is required");
        assert_eq!(
            refusals.to_string(),
            "username is required",
            "the one detail"
        );
        refusals.refuse_within("updatedFields", "a/b", RefusalReason::TooLong, "too long");
        let refused = refusals.into_result().expect_err("two refusals");
        assert_eq!(refused.to_string(), "2 members of the request were refused");
        let places: Vec<(&str, &str)> = refused
            .iter()
            .map(|refusal| (refusal.pointer.as_str(), refusal.reason.as_str()))
            .collect();
        assert_eq!(
            places,
            [
                ("/username", "required"),
                ("/updatedFields/a~1b", "too_long")
            ],
            "in the order they were refused, each where it is"
        );
    }

    #[test]
    fn every_reason_has_a_value_of_its_own() {
        let mut values: Vec<&str> = RefusalReason::ALL
            .iter()
            .map(|reason| reason.as_str())
            .collect();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), RefusalReason::ALL.len());
    }
}
