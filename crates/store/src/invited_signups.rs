//! Registrations made by the link of an invitation, on a server that confirms addresses:
//! the link counts one as a use when it is made, as it counts an account it lets in,
//! and lets its account in once it is confirmed; one that ends unconfirmed gives the
//! use back. Their links are held in the order acceptances hold them: what the link
//! hangs from (`link_locks::hold_for_letting_in`), the link, then the registration.

use fvoci_model::{AuditAction, InstanceRole, Invitation, WorkspaceRole};

use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;
use crate::invitations::{
    COLUMNS_OF_I, count_uses, hold_live_link, invitation_from_row, inviter_still, let_in,
    push_invites, take_use,
};
use crate::link_locks;
use crate::signups::{self, Confirmed, Proof, Signup, SignupStart, Started};
use crate::{Acceptance, Accepted, Inviter, Store, accounts};

/// What became of confirming a registration made by a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvitedConfirmation {
    /// The account was made and let in.
    Confirmed(Box<Confirmed>),
    /// The link lets nobody in any more (revoked, with the link it was made like, its
    /// board or its workspace's guests gone, its maker no longer inviting): the
    /// registration was removed and its use given back.
    Unavailable,
    /// The maker of the link is not what was found: nothing was changed, and whether
    /// the link lets anybody in is to be asked again.
    InviterChanged,
    /// No such registration, or it was confirmed meanwhile: nothing was changed.
    Gone,
}

impl Store {
    /// Stores `signup` as made by the live link of the handle's organization whose
    /// token has the digest `token_hash` at `now`, holding its name and address, and
    /// counts it as a use of the link, as [`Store::accept_invitation`] counts an
    /// account it lets in: a link for one guest lets nobody else in meanwhile. The
    /// registration's start is set to the link. Acceptances at once of one link wait
    /// for each other.
    ///
    /// The registration made before from the same browser, whose marker has the
    /// digest `replacing`, is removed with it: its name and address are free for this
    /// one, and the use of the link it was made by is given back, of this link too, so
    /// that a link for one guest takes the same person again. A registration that is
    /// refused replaces nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if an account or another registration holds
    /// the name or the address, or ([`crate::Taken::LinkUse`]) the link was used as
    /// often as it may be after all; [`StoreError::NoOrganization`] if the handle
    /// names none; and [`StoreError::Database`] if a statement fails. Nothing is
    /// stored or removed then.
    pub async fn reserve_invitation(
        &self,
        token_hash: &str,
        signup: &Signup,
        replacing: Option<&str>,
        now: i64,
        inviter: Inviter<'_>,
    ) -> Result<Acceptance, StoreError> {
        self.transaction(async |scope| {
            // The link of what is replaced may be another than this one.
            let replacing = match replacing {
                Some(browser) => Some((browser, link_locks::take_turn(scope).await?)),
                None => None,
            };
            let held = hold_live_link(scope, token_hash, now, inviter.standing, replacing);
            let link = match held.await? {
                Ok(link) => link,
                Err(refused) => return Ok(refused),
            };
            if let Some((browser, turn)) = replacing {
                signups::discard_of_browser(scope, browser, turn).await?;
            }
            let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
            let signup = Signup {
                start: SignupStart::Invitation {
                    id: link.id.clone(),
                    org_id,
                },
                ..signup.clone()
            };
            signups::hold_names(scope, &signup).await?;
            take_use(scope, &link.id).await?;
            Ok(Acceptance::Accepted(Accepted {
                user_id: signup.id,
                workspace_id: link.workspace_id,
                container_id: link.container_id,
                used: true,
            }))
        })
        .await
    }

    /// The link of the handle's organization with the identifier `invitation_id`,
    /// however it is: revoked, ended or used up.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn invitation_by_id(
        &self,
        invitation_id: &str,
    ) -> Result<Option<Invitation>, StoreError> {
        self.with_conn(async |scope| link_by_id(scope, invitation_id).await)
            .await
    }

    /// Confirms `signup`, made by a link of the handle's organization, which `proof`
    /// names at `now`: makes its account and lets it in by the link, as an acceptance
    /// does, counting no further use. The link need not be live: its use was taken
    /// while it was; it must still let somebody in otherwise (not revoked, nor the link
    /// it was made like; its board there and its workspace inviting guests, for a
    /// guest's link) and its maker be `inviter` still (`None` for one who may no
    /// longer invite). A confirmation by the server's operator of a link for members
    /// is recorded in the organization as theirs. All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if the account cannot take the registration's
    /// name, [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn confirm_invited_signup(
        &self,
        signup: &Signup,
        proof: Proof<'_>,
        inviter: Option<Inviter<'_>>,
        now: i64,
    ) -> Result<InvitedConfirmation, StoreError> {
        let SignupStart::Invitation { id, .. } = &signup.start else {
            return Err(StoreError::not_found("registration by a link"));
        };
        self.transaction(async |scope| {
            let Some(link) = hold_link(scope, id).await? else {
                return Ok(InvitedConfirmation::Gone);
            };
            let lets_in = lets_in(scope, &link).await?;
            let Some(inviter) = inviter.filter(|_| lets_in) else {
                // The use goes back with the registration that took it, and only then:
                // one confirmed or replaced meanwhile keeps it. The link is held.
                if !signups::take(scope, &signup.id, proof, now).await? {
                    return Ok(InvitedConfirmation::Gone);
                }
                count_uses(scope, &link.id, -1).await?;
                return Ok(InvitedConfirmation::Unavailable);
            };
            if !inviter_still(scope, &link, inviter.standing).await? {
                return Ok(InvitedConfirmation::InviterChanged);
            }
            if !signups::take(scope, &signup.id, proof, now).await? {
                return Ok(InvitedConfirmation::Gone);
            }
            let mut user = signup.to_account();
            user.email_verified_at = now.max(1);
            accounts::insert_account(scope, &mut user, InstanceRole::User).await?;
            let_in(scope, &link, &user.id, inviter.of_container).await?;
            let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
            if matches!(proof, Proof::Operator(_)) && link.role == WorkspaceRole::Member {
                let entry = AuditEntry::by(
                    &AuditActor::ServerOperator,
                    "",
                    AuditAction::EmailVerifiedByOperator,
                    &org_id,
                )
                .with_subject(&user.id);
                audit::record_entry_in(scope, &org_id, &entry).await?;
            }
            Ok(InvitedConfirmation::Confirmed(Box::new(Confirmed {
                user,
                org_id,
                workspace_id: link.workspace_id,
                container_id: link.container_id,
                start: Started::Invited,
            })))
        })
        .await
    }
}

/// The link with the identifier, of the scope's organization, however it is.
async fn link_by_id(
    scope: &mut Scope<'_>,
    invitation_id: &str,
) -> Result<Option<Invitation>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        COLUMNS_OF_I,
        " FROM invitations AS i WHERE i.id = ",
    ]);
    sql.bind(Id(invitation_id))
        .push_bind(" AND i.org_id = ", scope.org()?);
    scope
        .fetch_optional(sql)
        .await?
        .as_ref()
        .map(invitation_from_row)
        .transpose()
}

/// Holds the link with the identifier against its acceptances, after what it hangs
/// from, as an acceptance holds them; the link, if it is there.
async fn hold_link(
    scope: &mut Scope<'_>,
    invitation_id: &str,
) -> Result<Option<Invitation>, StoreError> {
    let Some(link) = link_by_id(scope, invitation_id).await? else {
        return Ok(None);
    };
    // A board that is gone took its links along.
    if !link_locks::hold_for_letting_in(scope, &link, None).await? {
        return Ok(None);
    }
    let mut sql = scope.sql("SELECT i.id FROM invitations AS i WHERE i.id = ");
    sql.bind(Id(invitation_id))
        .push_bind(" AND i.org_id = ", scope.org()?);
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE OF i");
    }
    Ok(scope.fetch_optional(sql).await?.map(|_| link))
}

/// Whether the held link lets somebody in, apart from its end and its uses: it is not
/// revoked, nor is the link it was made like (whose revocation leaves the children
/// that were used), and its workspace invites guests if it is for one.
async fn lets_in(scope: &mut Scope<'_>, link: &Invitation) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM invitations AS i WHERE i.id = ");
    sql.bind(Id(&link.id))
        .push_bind(" AND i.org_id = ", scope.org()?)
        .push_bind(" AND i.revoked_at = ", 0_i64)
        .push(
            " AND NOT EXISTS (SELECT 1 FROM invitations AS p WHERE p.id = i.parent_id \
             AND p.org_id = i.org_id AND p.revoked_at <> 0)",
        );
    push_invites(&mut sql);
    Ok(scope.fetch_optional(sql).await?.is_some())
}
