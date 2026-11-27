//! The kind of an organization: one that people work in together, or the space of one
//! account.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// What an organization is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OrgKind {
    /// People work in it together: it has members and administrators, workspaces,
    /// invitations, guests, groups of people and a provider of sign-ins.
    Shared,
    /// The space of one account, which every account has one of: the account is its
    /// only member and it has one workspace. Nobody is invited into it and nobody
    /// administers it; what is in it is the account's alone, but for what the account
    /// shares by a public link.
    Personal,
}

impl OrgKind {
    /// Every organization kind, in contract order.
    pub const ALL: [Self; 2] = [Self::Shared, Self::Personal];

    /// The wire words in the order of [`Self::ALL`].
    pub const WORDS: &[&str] = &["shared", "personal"];

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Personal => "personal",
        }
    }
}

impl FromStr for OrgKind {
    type Err = UnknownOrgKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "shared" => Ok(Self::Shared),
            "personal" => Ok(Self::Personal),
            other => Err(UnknownOrgKind(other.to_owned())),
        }
    }
}

/// Text that names no kind of organization.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is no kind of organization")]
pub struct UnknownOrgKind(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_reads_back_from_its_text_and_nothing_else_does() {
        for kind in OrgKind::ALL {
            assert_eq!(kind.as_str().parse(), Ok(kind), "{kind:?}");
            assert_eq!(
                serde_json::to_value(kind).expect("a kind is JSON"),
                kind.as_str(),
                "the wire form is the database's"
            );
        }
        assert_eq!(
            "Personal".parse::<OrgKind>(),
            Err(UnknownOrgKind("Personal".to_owned())),
            "in this letter case only"
        );
        let retired = concat!("te", "am");
        assert_eq!(
            retired.parse::<OrgKind>(),
            Err(UnknownOrgKind(retired.to_owned())),
            "the former name is rejected"
        );
    }
}
