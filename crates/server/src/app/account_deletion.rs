//! Deleting an account: what its person is shown, the asking, and taking it back.

use std::net::IpAddr;

use fvoci_model::{
    ACCOUNT_DELETION_GRACE_DAYS, AccountDeletion, AccountDeletionAsked, DeleteAccountRequest,
    DeletionProof, Leaving, LeavingOrganization, OrgKind, Session, User,
};
use fvoci_store::{DeletionAsking, DeletionStanding, SessionTaken};

use super::App;
use super::auth::WrongPassword;
use super::oidc_pending::require_fresh_sign_in;
use crate::auth::now_millis;
use crate::error::{AppError, Reason};

/// The wait between the asking and the erasing, in milliseconds.
const GRACE_MILLIS: i64 = ACCOUNT_DELETION_GRACE_DAYS * 24 * 60 * 60 * 1000;

impl App {
    /// What deleting the account of the session comes to, as things are now: what
    /// stands in its way and which organizations go with it; and, for an account that
    /// waits already (`leaving`), until when. How much is in an organization that goes
    /// is told to a session that opens it only (`App::enter_org_as_admin`).
    ///
    /// # Errors
    ///
    /// Returns a 404 if there is no such account to sign in to, and the store's error
    /// if it cannot be read.
    pub async fn account_deletion(
        &self,
        session: &Session,
        leaving: Option<Leaving>,
    ) -> Result<AccountDeletion, AppError> {
        let user = self.store().account_by_id(&session.user_id).await?;
        let standing = self
            .store()
            .account_deletion_standing(&session.user_id)
            .await?;
        let mut organizations = Vec::with_capacity(standing.alone_in.len() + 1);
        // The account's own space is its owner's to count, and nobody enters it as an
        // administrator: asked of a session that opens it, as its workspace is.
        if let Some(own_space) = standing.own_space {
            let takes = SessionTaken::of_space(&user.password_hash);
            if takes.opened_by(session.into()) {
                let within = self.in_organization(&own_space.id)?;
                if let Some(counted) = within.store().leaving_organization(own_space).await? {
                    organizations.push(counted);
                }
            } else {
                organizations.push(LeavingOrganization::closed(own_space, OrgKind::Personal));
            }
        }
        for organization in standing.alone_in {
            let within = match self.enter_org_as_admin(session, &organization.id).await {
                Ok(within) => within,
                Err(error) if error.reason() == Some(Reason::SsoRequired) => None,
                Err(error) => return Err(error),
            };
            match within {
                Some(within) => {
                    if let Some(counted) = within.store().leaving_organization(organization).await?
                    {
                        organizations.push(counted);
                    }
                }
                None => {
                    organizations.push(LeavingOrganization::closed(organization, OrgKind::Shared));
                }
            }
        }
        Ok(AccountDeletion {
            requested_at: leaving.map(|leaving| leaving.requested_at),
            delete_after: leaving.map(|leaving| leaving.delete_after),
            grace_days: ACCOUNT_DELETION_GRACE_DAYS,
            proof: proof_of(&user),
            last_instance_admin: standing.last_instance_admin,
            sole_admin_of: standing.sole_admin_of,
            organizations,
        })
    }

    /// Puts the account of the session out of use, to be erased after the wait: its
    /// sessions end, this one too, and its websocket connections close. The request
    /// names the account, and shows that it is the person's: by the account's password,
    /// or, for an account that has none, by a session made through a provider no more
    /// than five minutes ago (`FRESH_SIGN_IN_MILLIS`).
    ///
    /// # Errors
    ///
    /// Returns a 400 naming `username` if that is not the account's name, and one
    /// naming `password` (reason `invalid_credentials`) for a wrong password, with the
    /// 429 of too many; a 403 (reason `sign_in_again`) for an account without a
    /// password whose session was not made through a provider just now; a 401 (reason
    /// `sso_required`, naming it) if an organization that would go with the account
    /// is one the session does not open; a 409 if the
    /// account is the instance's only administrator (reason `last_admin`), or the only
    /// administrator of an organization that stays, one that has other people or the
    /// instance's own (reason `last_org_admin`, naming the organization), or was
    /// changed meanwhile.
    pub async fn request_account_deletion(
        &self,
        session: &Session,
        request: &DeleteAccountRequest,
        address: Option<IpAddr>,
    ) -> Result<AccountDeletionAsked, AppError> {
        let user = self.store().get_user_by_id(&session.user_id).await?;
        if request.username.to_lowercase() != user.username.to_lowercase() {
            return Err(
                AppError::bad_request("the user name is not the account's").with_field("username")
            );
        }
        // An organization that has nobody else goes with the account: that is the
        // organization's, and asked of a session that opens it. Leaving one is the
        // account's own.
        let standing = self.store().account_deletion_standing(&user.id).await?;
        for organization in &standing.alone_in {
            self.enter_org_as_admin(session, &organization.id).await?;
        }
        let now = now_millis();
        match proof_of(&user) {
            DeletionProof::Password => {
                let wrong = WrongPassword {
                    words: "the password is wrong",
                    field: "password",
                };
                self.confirm_password(&user, &request.password, wrong, address)
                    .await?;
            }
            DeletionProof::SignIn => require_fresh_sign_in(session, now, "delete the account")?,
        }
        let asking = self
            .store()
            .request_account_deletion(
                &user.id,
                &user.password_hash,
                now,
                now.saturating_add(GRACE_MILLIS),
            )
            .await
            .map_err(|error| AppError::from(error).context("asking to delete an account"))?;
        match asking {
            DeletionAsking::Asked(leaving, ended) => {
                self.ws().close(ended.into_scope());
                let own_space_id = standing.own_space.as_ref().map(|space| space.id.as_str());
                self.end_readers_of_own_space(&user.id, own_space_id).await;
                tracing::info!(
                    user_id = %user.id,
                    delete_after = leaving.delete_after,
                    "an account was asked to be deleted"
                );
                Ok(AccountDeletionAsked {
                    delete_after: leaving.delete_after,
                })
            }
            DeletionAsking::Refused(standing) => Err(refused(&standing)),
            DeletionAsking::Changed => Err(AppError::conflict(
                "the account was changed in the meantime; sign in again",
            )),
        }
    }

    /// Ends what connections follow of the boards of the account's own space by a
    /// public link: the links open nothing while the account waits to be deleted. The
    /// asking stands whatever becomes of this; a connection that is missed is told
    /// nothing more once what it was let in by is checked again.
    async fn end_readers_of_own_space(&self, user_id: &str, own_space_id: Option<&str>) {
        let Some(own_space_id) = own_space_id else {
            return;
        };
        let ended = async {
            self.in_organization(own_space_id)?
                .end_shared_readers(None)
                .await
        };
        if let Err(error) = ended.await {
            tracing::warn!(user_id, %error, "the readers of an own space's links were not ended");
        }
    }

    /// Takes back the asking to delete the account: it is in use again as it was.
    ///
    /// # Errors
    ///
    /// Returns a 409 if the account does not wait any more: it is in use, or its
    /// erasing began.
    pub async fn cancel_account_deletion(&self, user_id: &str) -> Result<(), AppError> {
        if !self
            .store()
            .cancel_account_deletion(user_id, now_millis())
            .await?
        {
            return Err(AppError::conflict(
                "the account does not wait to be deleted",
            ));
        }
        tracing::info!(user_id, "the asking to delete an account was taken back");
        Ok(())
    }
}

/// How the account shows that a request to delete it is its person's.
fn proof_of(user: &User) -> DeletionProof {
    if user.password_hash.is_empty() {
        DeletionProof::SignIn
    } else {
        DeletionProof::Password
    }
}

/// What a request is refused with that would leave something without anybody to run
/// it: the instance first, then the first of the organizations, by identifier.
pub(crate) fn refused(standing: &DeletionStanding) -> AppError {
    if standing.last_instance_admin {
        return AppError::conflict("the instance must keep an administrator")
            .with_reason(Reason::LastAdmin);
    }
    match standing.sole_admin_of.first() {
        Some(organization) => AppError::last_org_admin(&organization.id),
        None => AppError::internal("a deletion was refused with nothing in its way"),
    }
}
