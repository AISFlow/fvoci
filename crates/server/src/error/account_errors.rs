//! The refusals of deleting an account, and of the session of one that waits.

use super::{AppError, ErrorKind, Reason};

impl AppError {
    /// The 401 of a session whose account waits to be deleted: it is good for the way
    /// back (`getAccountDeletion`, `cancelAccountDeletion`, `logout`) and opens nothing
    /// else.
    pub fn account_deletion_pending() -> Self {
        Self::new(
            ErrorKind::Unauthorized,
            "the account waits to be deleted; take that back to use it",
        )
        .with_reason(Reason::AccountDeletionPending)
    }

    /// The 409 of an account that is not deleted while it is the only administrator
    /// of the organization `org_id`, which stays when it goes: one that has other
    /// people in it, or the instance's own. The answer names the organization.
    pub fn last_org_admin(org_id: &str) -> Self {
        let mut error = Self::new(
            ErrorKind::Conflict,
            "the organization must keep an administrator: make somebody else one first",
        )
        .with_reason(Reason::LastOrgAdmin);
        error.more().org_id = Some(org_id.to_owned());
        error
    }
}
