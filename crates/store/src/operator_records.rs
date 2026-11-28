//! What the server's operator does to a person's password, recorded in every
//! organization the person is a member of: one record each, as the operator's, in the
//! transaction of what it records. The one place besides the directory that writes
//! across organizations, with the system's reach, narrowed by the person's
//! memberships.

use fvoci_model::{AuditAction, OrgKind};

use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{Id, Scope};
use crate::error::StoreError;
use crate::sessions::PasswordSet;
use crate::{Store, directory, password_reset, users};

impl Store {
    /// Sets the password hash of the user with the given name, in any letter case, and
    /// deletes their sessions, so that nobody stays signed in with the old password. For
    /// a password the server's operator sets: recorded as theirs in every organization
    /// the user is a member of ([`AuditAction::PasswordSetByOperator`]), with no word
    /// of the password. Returns the identifier of that user and the ended sessions.
    ///
    /// All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::UserNotFound`] if no user has that name.
    pub async fn reset_user_password(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<PasswordSet, StoreError> {
        self.system()
            .transaction(async |scope| {
                let set = users::reset_password(scope, username, password_hash).await?;
                let user_id = &set.user_id;
                if let Some(space) = directory::space_of(scope, user_id).await? {
                    scope.invalidate_visibility_of(&space)?;
                }
                record_in_organizations_of(scope, user_id, AuditAction::PasswordSetByOperator)
                    .await?;
                Ok(set)
            })
            .await
    }

    /// Stores a reset credential under `digest` for the user `user_id`, as
    /// [`Store::create_password_reset`] does, on the word of the server's operator (the
    /// administration socket): in the same transaction, recorded as theirs in every
    /// organization the user is a member of ([`AuditAction::PasswordResetLinkMade`]),
    /// one record each, with neither the link nor its token, and not which
    /// organization asked. Says whether it stored one: nothing is stored or recorded for
    /// an account that is not in use or has no password.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is stored then.
    pub async fn create_password_reset_by_operator(
        &self,
        user_id: &str,
        digest: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        // As create_password_reset: expired credentials go first, in their own
        // statement, before the user's row is taken.
        self.with_write_conn(async |scope| {
            password_reset::delete_expired_password_resets(scope, now).await
        })
        .await?;
        self.system()
            .transaction(async |scope| {
                if !password_reset::insert_password_reset(scope, user_id, None, digest, now).await?
                {
                    return Ok(false);
                }
                record_in_organizations_of(scope, user_id, AuditAction::PasswordResetLinkMade)
                    .await?;
                Ok(true)
            })
            .await
    }
}

/// Records `action` by the server's operator, done to the user `user_id`, in every
/// organization in use that they are a member of and people work in, one record each,
/// of no workspace: for its administrators to read. Not in the account's own space,
/// which has none.
/// Needs the system's reach: the wall hides the other organizations.
async fn record_in_organizations_of(
    scope: &mut Scope<'_>,
    user_id: &str,
    action: AuditAction,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT m.org_id FROM organization_members AS m \
         JOIN organizations AS o ON o.id = m.org_id WHERE m.user_id = ",
    );
    sql.bind(Id(user_id))
        .push(" AND o.deleted_at IS NULL AND o.is_system = false")
        .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
        .push(" ORDER BY m.org_id");
    let organizations: Vec<String> = scope.all(sql).await?;
    for org_id in &organizations {
        let entry =
            AuditEntry::by(&AuditActor::ServerOperator, "", action, org_id).with_subject(user_id);
        audit::record_entry_in(scope, org_id, &entry).await?;
    }
    Ok(())
}
