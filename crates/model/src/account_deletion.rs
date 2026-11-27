//! Deleting an account: what its person asks, and what they are shown before and
//! while it waits.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::org_kind::OrgKind;
use crate::refusal::{RefusalReason, Refusals};

/// How long an account waits between the asking and the erasing, in days: until then
/// its person may take the asking back.
pub const ACCOUNT_DELETION_GRACE_DAYS: i64 = 30;

/// A request to delete the account of the session.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeleteAccountRequest {
    /// The account's login name, typed out: that the person means this account.
    pub username: String,
    /// The account's password; left out, or empty, by an account that has none. Not
    /// `null`, as no member that is not optional is.
    pub password: String,
}

impl DeleteAccountRequest {
    /// Checks that the account is named. Whether the name is the account's, and the
    /// password, are the account's to judge.
    ///
    /// # Errors
    ///
    /// Returns `username` if it is empty.
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if self.username.is_empty() {
            refused.refuse("username", RefusalReason::Required, "username is required");
        }
        refused.into_result()
    }
}

impl fmt::Debug for DeleteAccountRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeleteAccountRequest")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// Until when an account that was asked to be deleted waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leaving {
    /// When the deletion was asked, in milliseconds since the Unix epoch.
    pub requested_at: i64,
    /// When the account is erased at the earliest.
    pub delete_after: i64,
}

/// How an account shows that the request to delete it is its person's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeletionProof {
    /// By its password, sent with the request.
    Password,
    /// By a session made through a provider just now: the account has no password.
    SignIn,
}

/// An organization named to its member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationName {
    /// The organization's identifier.
    pub id: String,
    /// Its name; empty if it was given none.
    pub name: String,
}

/// An organization that goes with the account of its only member, and how much is in
/// it: the account's own space, and an organization of people that has nobody else. The
/// numbers are left out for an organization the session that asks does not
/// open (one that takes only sessions made through its provider).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeavingOrganization {
    /// The organization's identifier.
    pub id: String,
    /// What it is: `personal` for the account's own space, which always goes with it.
    pub kind: OrgKind,
    /// Its name; empty if it was given none, as the account's own space always is.
    pub name: String,
    /// How many boards it has in use, templates not counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub boards: Option<i64>,
    /// How many wikis it has in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub wikis: Option<i64>,
    /// How many diaries it has in use: only the account's own space has any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub diaries: Option<i64>,
    /// How many courses it has in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub courses: Option<i64>,
    /// How many files its boards, wikis, diaries and courses in use hold.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub files: Option<i64>,
}

impl LeavingOrganization {
    /// The organization named, of the `kind`, with nothing told of what is in it.
    #[must_use]
    pub fn closed(organization: OrganizationName, kind: OrgKind) -> Self {
        Self {
            id: organization.id,
            kind,
            name: organization.name,
            boards: None,
            wikis: None,
            diaries: None,
            courses: None,
            files: None,
        }
    }
}

/// What deleting the account of the session comes to: what stands in its way, what
/// goes with it, and, once it was asked, until when it waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountDeletion {
    /// When the deletion was asked, in milliseconds since the Unix epoch; left out
    /// while it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub requested_at: Option<i64>,
    /// When the account is erased at the earliest, in milliseconds since the Unix
    /// epoch; left out while the deletion is not asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub delete_after: Option<i64>,
    /// How many days an account waits between the asking and the erasing.
    pub grace_days: i64,
    /// How the request shows that it is the person's.
    pub proof: DeletionProof,
    /// Whether the account is the server's only administrator, who stays: the server
    /// keeps one.
    pub last_instance_admin: bool,
    /// The organizations the account is the only administrator of that stay when it
    /// goes: those that have other people in them, and the server's own first
    /// organization, which goes with no account. Somebody else is made an
    /// administrator of each before the account can be deleted.
    pub sole_admin_of: Vec<OrganizationName>,
    /// The organizations that have nobody but the account, which go with it, and
    /// everything in them: the account's own space (`kind` `personal`) always, and
    /// every other organization it is alone in. The server's own first organization
    /// is never one.
    pub organizations: Vec<LeavingOrganization>,
}

/// The answer to a request to delete the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountDeletionAsked {
    /// When the account is erased at the earliest, in milliseconds since the Unix
    /// epoch.
    pub delete_after: i64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_member_left_out_is_empty_and_null_is_refused() {
        let request: DeleteAccountRequest =
            serde_json::from_value(json!({"username": "kim"})).expect("the password left out");
        assert_eq!(request.password, "", "no password");
        for member in ["username", "password"] {
            let mut body = json!({"username": "kim", "password": "secret"});
            body[member] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<DeleteAccountRequest>(body).is_err(),
                "{member}: null is no string"
            );
        }
    }
}
