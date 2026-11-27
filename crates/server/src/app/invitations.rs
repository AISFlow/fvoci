//! Invitations: making the link that lets somebody into a workspace, and letting in
//! whoever comes with one.
//!
//! The token of a link is made here, told once to who makes the link, and kept as its
//! digest. A link lets somebody in while it is live (`fvoci_store::Store::live_invitation`)
//! and while who made it may still invite: that is asked anew every time the link is
//! used, of the facts every other check reads (`App::actor`), and an acceptance is
//! stored only if the maker is still what was found then, where it is written
//! (`fvoci_store::Inviter`).

mod mail;

use std::net::IpAddr;

use fvoci_model::User;
use fvoci_model::{
    AcceptedInvitation, CheckedAccount, ContainerKind, ContainerMember, ContainerRole,
    GUEST_INVITATION_USES, GuestInvitationsChange, INVITATION_LIFETIME_MILLIS, Invitation,
    InvitationPreview, Workspace, WorkspaceRole,
};
use fvoci_store::{
    Acceptance, Accepted, Acceptor, Inviter, InviterOfContainer, MailedTo, NewLink, Signup,
    Standing, StoreError, Taken,
};

use super::auth::account_error;
use super::language::Language;
use super::signups::SignupMarker;
use super::{App, Asker, ContainerAccess};
use crate::auth::now_millis;
use crate::auth::secret::{new_secret, token_digest};
use crate::collab::AccessScope;
use crate::error::AppError;
use crate::permissions::{self, Actor, ContainerScope, WorkspacePermission};

/// How often an acceptance is tried against a maker of the link who is changed
/// meanwhile.
pub(super) const ATTEMPTS: usize = 3;

/// What the maker of a link was found to be when their link was found to let somebody
/// in: what the acceptance holds them to (`fvoci_store::Inviter`).
#[derive(Debug)]
pub(super) struct Maker {
    workspace_role: Option<WorkspaceRole>,
    /// Their membership of the board of a guest's link.
    member: Option<ContainerMember>,
    team_role: Option<fvoci_model::ContainerRole>,
    of_container: InviterOfContainer,
}

impl Maker {
    /// What the acceptance holds the maker `user_id` to.
    pub(super) fn inviter<'a>(&'a self, user_id: &'a str) -> Inviter<'a> {
        Inviter {
            standing: Standing {
                user_id,
                workspace_role: self.workspace_role,
                member: self.member.as_ref(),
                team_role: self.team_role,
            },
            of_container: self.of_container,
        }
    }
}

/// Who accepts a link.
#[derive(Debug, Clone, Copy)]
pub enum Accepting<'a> {
    /// The user of the request's session, and the provider it was made through.
    SignedIn(Asker<'a>),
    /// Somebody without an account, who asks for this one, from a screen in
    /// `language`.
    NewAccount {
        /// The account asked for.
        account: &'a CheckedAccount,
        /// The language tag of the screen, for the mail that confirms its address.
        language: &'a str,
        /// The secret of the browser's cookie, if it has one of a registration that
        /// waits ([`SignupMarker`]): this account replaces that registration.
        browser: Option<&'a str>,
    },
}

/// What a new link is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inviting<'a> {
    /// Members of the workspace: the link is used as often as it is until it ends.
    Members,
    /// A guest of one board of the workspace, in a role on it: the link is used once.
    Guest {
        /// The board.
        container_id: &'a str,
        /// The guest's role on the board.
        role: ContainerRole,
    },
}

impl App {
    /// Makes a link that lets whoever accepts it into the workspace, for seven days,
    /// and returns it with its token: the one time the token is told. The handle is
    /// the one the user entered the workspace with, and the caller has asked whether
    /// they may invite so.
    ///
    /// # Errors
    ///
    /// Returns a 404 if the workspace is not in use or has no such board to invite a
    /// guest to, and the store's error if the link cannot be stored.
    pub async fn invite(
        &self,
        workspace_id: &str,
        created_by: &str,
        inviting: Inviting<'_>,
    ) -> Result<Invitation, AppError> {
        self.store_link(workspace_id, created_by, inviting, None, None)
            .await
    }

    /// Stores a new link that ends at `expires_at`, or seven days after it is made for
    /// `None`, `mailed` for one recipient of a link if it is, and returns it with its
    /// token. The clock is read once: a link's lifetime is the same to the millisecond
    /// whenever it is made.
    async fn store_link(
        &self,
        workspace_id: &str,
        created_by: &str,
        inviting: Inviting<'_>,
        expires_at: Option<i64>,
        mailed: Option<MailedTo<'_>>,
    ) -> Result<Invitation, AppError> {
        let now = now_millis();
        let expires_at =
            expires_at.unwrap_or_else(|| now.saturating_add(INVITATION_LIFETIME_MILLIS));
        let token = new_secret();
        let (guest_of, max_uses) = match inviting {
            Inviting::Members => (None, None),
            Inviting::Guest { container_id, role } => {
                (Some((container_id, role)), Some(GUEST_INVITATION_USES))
            }
        };
        let link = NewLink {
            workspace_id,
            guest_of,
            token_hash: &token_digest(&token),
            created_by,
            now,
            expires_at,
            max_uses,
            mailed,
        };
        let mut invitation = match self.store().create_invitation(&link).await {
            Ok(invitation) => invitation,
            Err(error) if error.is_not_found() => {
                return Err(AppError::not_found(match inviting {
                    Inviting::Members => "workspace",
                    Inviting::Guest { .. } => "board",
                }));
            }
            Err(error) => {
                return Err(AppError::from(error).context("unable to store an invitation"));
            }
        };
        tracing::info!(
            invitation_id = %invitation.id,
            workspace_id,
            container_id = invitation.container_id.as_deref().unwrap_or_default(),
            user_id = created_by,
            role = invitation.role.as_str(),
            "a link was made"
        );
        invitation.token = Some(token);
        Ok(invitation)
    }

    /// The links into the workspace that the user sees (the live ones, and a used one
    /// while an unused child of it is out): all of them for who invites its members,
    /// their own for anybody else. The newest first, and none
    /// with its token.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the links cannot be read.
    pub async fn invitations(
        &self,
        workspace_id: &str,
        user_id: &str,
        actor: Actor,
    ) -> Result<Vec<Invitation>, AppError> {
        let own = (!sees_every_link(actor)).then_some(user_id);
        Ok(self
            .store()
            .live_invitations(workspace_id, own, now_millis())
            .await?)
    }

    /// Revokes a link into the workspace that is neither revoked nor ended, used or
    /// not, with its children nobody used: any of them for who invites its members, one
    /// of their own for anybody else. The link lets nobody in from then on.
    ///
    /// # Errors
    ///
    /// Returns one and the same 404 if there is no such live link, or it is not the
    /// user's to revoke.
    pub async fn revoke_invitation(
        &self,
        workspace_id: &str,
        invitation_id: &str,
        user_id: &str,
        actor: Actor,
    ) -> Result<(), AppError> {
        let own = (!sees_every_link(actor)).then_some(user_id);
        let revoked = fvoci_model::is_id(invitation_id)
            && self
                .store()
                .revoke_invitation(workspace_id, invitation_id, own, now_millis())
                .await?;
        if !revoked {
            return Err(AppError::not_found("invitation"));
        }
        tracing::info!(invitation_id, workspace_id, user_id, "a link was revoked");
        Ok(())
    }

    /// Turns the invitations of guests to the boards of the workspace on or off, on
    /// behalf of the user, and returns the workspace. Turned off, the links that invite
    /// guests are revoked with it, and stay so when it is turned on again; who is a
    /// guest already stays one. The caller has asked whether the user may.
    ///
    /// # Errors
    ///
    /// Returns a 404 if the workspace is not in use.
    pub async fn set_guest_invitations(
        &self,
        workspace_id: &str,
        change: GuestInvitationsChange,
        user_id: &str,
    ) -> Result<Workspace, AppError> {
        match self
            .store()
            .set_guest_invitations(workspace_id, change, user_id)
            .await
        {
            Ok(workspace) => {
                tracing::info!(
                    workspace_id,
                    user_id,
                    enabled = change.enabled,
                    "the invitations of guests were switched"
                );
                Ok(workspace)
            }
            Err(error) if error.is_not_found() => Err(AppError::not_found("workspace")),
            Err(error) => Err(error.into()),
        }
    }

    /// What the user is to a board of the workspace, and whether they invite guests to
    /// it (`permissions::invites_guests`): what making a link for a guest takes, and
    /// what such a link needs of its maker for as long as it is to let anybody in.
    /// `None` for no such board in the workspace.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read.
    pub async fn guest_inviter(
        &self,
        user_id: &str,
        workspace_id: &str,
        container_id: &str,
    ) -> Result<Option<(ContainerAccess, bool)>, AppError> {
        let Some(access) = self.container_access(user_id, container_id).await? else {
            return Ok(None);
        };
        if access.container.workspace_id != workspace_id {
            return Ok(None);
        }
        let workspace = match self.store().get_workspace(workspace_id).await {
            Ok(workspace) => workspace,
            Err(error) if error.is_not_found() => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let invites = permissions::invites_guests(
            access.grant,
            ContainerScope::of(&access.container),
            &workspace,
        );
        Ok(Some((access, invites)))
    }

    /// The live link `token` is of, with the application behind the wall of the
    /// link's organization; `None` if the token is of no link that lets anybody in:
    /// no link at all, one that ended, was used up or revoked, or one whose maker may
    /// no longer invite. A link that only the registration of the browser of
    /// `replacing` (the digest of its marker) uses up lets that browser in still: what
    /// it does next replaces the registration.
    async fn usable_invitation(
        &self,
        token: &str,
        replacing: Option<&str>,
    ) -> Result<Option<(Self, Invitation, Maker)>, AppError> {
        let token_hash = token_digest(token);
        let now = now_millis();
        let Some(org_id) = self
            .store()
            .org_of_live_invitation(&token_hash, now, replacing)
            .await?
        else {
            return Ok(None);
        };
        let Some(app) = self.enter_org_for_sign_in(&org_id) else {
            return Err(AppError::internal(
                "the organization of an invitation as the store has it is no identifier",
            ));
        };
        let live = app.store().live_invitation(&token_hash, now, replacing);
        let Some(invitation) = live.await? else {
            return Ok(None);
        };
        let Some(maker) = app.inviter_who_may_still_invite(&invitation).await? else {
            return Ok(None);
        };
        Ok(Some((app, invitation, maker)))
    }

    /// Who made the link, as they are now, if they may make it now: a link outlives
    /// nothing that its maker lost. A link for members is a workspace administrator's;
    /// one for a guest is of who adds members to its board, while the workspace invites
    /// guests.
    async fn inviter_who_may_still_invite(
        &self,
        invitation: &Invitation,
    ) -> Result<Option<Maker>, AppError> {
        let (maker, workspace_id) = (&invitation.created_by, &invitation.workspace_id);
        match (invitation.role, &invitation.container_id) {
            (WorkspaceRole::Member, _) => {
                let actor = self.actor(maker, workspace_id).await?;
                let invites =
                    permissions::allows_in_workspace(actor, WorkspacePermission::InviteMembers);
                Ok(invites.then_some(Maker {
                    workspace_role: actor.workspace,
                    member: None,
                    team_role: None,
                    of_container: InviterOfContainer::Member,
                }))
            }
            (WorkspaceRole::Guest, Some(container_id)) => Ok(self
                .guest_inviter(maker, workspace_id, container_id)
                .await?
                .filter(|(_, invites)| *invites)
                .map(|(access, _)| Maker {
                    workspace_role: access.actor.workspace,
                    // A manager who does not read the board puts nobody on it off the
                    // record, by a link as little as by adding them.
                    of_container: if access.grant.manages_only() {
                        InviterOfContainer::ManagerOnly
                    } else {
                        InviterOfContainer::Member
                    },
                    member: access.member,
                    team_role: access.team_role,
                })),
            (WorkspaceRole::Guest, None) | (WorkspaceRole::Admin, _) => Ok(None),
        }
    }

    /// What the link `token` is of shows to whoever holds it, signed in or not: the name
    /// of the workspace it leads into and of who made it, for a guest the title of the
    /// board, and nothing else of any of them.
    ///
    /// `address` is where the request comes from: a look at a link is counted against
    /// it as an acceptance is, so that tokens are not tried without end. `browser` is
    /// the secret of the browser's cookie of a registration that waits, if it has one:
    /// a link that only that registration uses up is shown to it still, to register
    /// by again.
    ///
    /// # Errors
    ///
    /// Returns the 404 of [`App::accept_invitation`] for a token that lets nobody in,
    /// whatever is wrong with it, and a 429 for too many attempts from the address.
    pub async fn preview_invitation(
        &self,
        token: &str,
        browser: Option<&str>,
        address: Option<IpAddr>,
    ) -> Result<InvitationPreview, AppError> {
        let _attempt = self.admit_registration(address)?;
        let replacing = self.replaced_by(browser);
        let usable = self.usable_invitation(token, replacing.as_deref());
        let Some((app, invitation, _)) = usable.await? else {
            return Err(AppError::invitation_unavailable());
        };
        let workspace = app.workspace(&invitation.workspace_id).await?;
        let inviter = app.user(&invitation.created_by).await?;
        let container = match &invitation.container_id {
            Some(container_id) => match app.store().get_container(container_id).await {
                Ok(container) => Some(container),
                // Gone since the link was read: with it the link.
                Err(error) if error.is_not_found() => {
                    return Err(AppError::invitation_unavailable());
                }
                Err(error) => return Err(error.into()),
            },
            None => None,
        };
        Ok(InvitationPreview {
            workspace_name: workspace.title,
            inviter_name: if inviter.display_name.is_empty() {
                inviter.username
            } else {
                inviter.display_name
            },
            container_kind: container.as_ref().map(|container| container.kind),
            container_title: container.map(|container| container.title),
        })
    }

    /// Lets somebody in by the link `token` is of, and says where: the user of a
    /// session, or a new account that is made with it, in one transaction with the
    /// counting of the use. An account that has all the link gives already is let in
    /// again, and uses nothing of the link.
    ///
    /// The maker of the link is looked at again where the acceptance is written, once
    /// the link is held: a link whose maker was taken out of the workspace while the
    /// acceptance waited lets nobody in, as a look at it says by then.
    ///
    /// A new account from the browser of a registration that waits for its address
    /// replaces that registration, in the same transaction: its name and address are
    /// free for the account, and the use it took of a link is given back, of this link
    /// too. An acceptance that is refused replaces nothing.
    ///
    /// `address` is where the request comes from: acceptances are counted against it
    /// as registrations are, and one of an address is under way at a time.
    ///
    /// An account that comes into the workspace as a member by it is given its welcome
    /// board there, if it was given none before (`App::welcome_member`): in the
    /// language of the account's settings; where they name none, in the language of
    /// the screen a new account was made on, if the server writes in it, or else the
    /// one `accept_language`, the request's `Accept-Language`, prefers.
    ///
    /// # Errors
    ///
    /// Returns one and the same 404 (reason `invitation_unavailable`) for a token that
    /// lets nobody in, whatever is wrong with it; a conflict error if the name or the
    /// email address of a new account is taken; a 429 if there are too many attempts
    /// from the address; and the store's error if nothing could be stored.
    pub async fn accept_invitation(
        &self,
        token: &str,
        accepting: Accepting<'_>,
        address: Option<IpAddr>,
        accept_language: Option<&str>,
    ) -> Result<Admission, AppError> {
        let attempt = self.admit_registration(address)?;
        let mut new_user = None;
        let mut settled = None;
        for _ in 0..ATTEMPTS {
            match self.try_accepting(token, accepting, &mut new_user).await? {
                Tried::Settled(found) => {
                    let Settled {
                        app,
                        accepted,
                        waiting,
                    } = *found;
                    settled = Some((app, accepted, waiting));
                    break;
                }
                // Whether the maker may still invite is asked again, of what they are now.
                Tried::Again => {}
            }
        }
        // Its maker was changed every time it was tried: as good as no link.
        let Some((app, accepted, waiting)) = settled else {
            return Err(AppError::invitation_unavailable());
        };
        if let Some((signup, browser)) = waiting {
            tracing::info!(
                signup_id = %signup.id,
                workspace_id = %accepted.workspace_id,
                "an invitation was accepted by a registration that waits for its address"
            );
            let app = self.clone();
            self.spawn_background(async move { app.mail_signup_link(&signup, None).await });
            return Ok(Admission::Waiting(browser));
        }
        if new_user.is_some() {
            self.metrics().registered();
        }
        app.announce_admission(&accepted).await;
        // A guest's link names a board; a link for members names none.
        if accepted.used && accepted.container_id.is_none() {
            // The acceptance is done with: another from the address waits no longer.
            drop(attempt);
            // An account made here is written to in the language of the screen it was
            // made on, as one whose address is confirmed first is, if the server
            // writes in it: or else as the header has it.
            let language = match accepting {
                Accepting::NewAccount { language, .. } => Language::written_in(language)
                    .map(Language::tag)
                    .or(accept_language),
                Accepting::SignedIn(_) => accept_language,
            };
            app.welcome_member(&accepted.user_id, &accepted.workspace_id, language)
                .await;
        }
        tracing::info!(
            user_id = %accepted.user_id,
            workspace_id = %accepted.workspace_id,
            new_account = new_user.is_some(),
            used = accepted.used,
            "an invitation was accepted"
        );
        let container_kind = app.kind_of_admitted(accepted.container_id.as_deref()).await;
        Ok(Admission::LetIn(AcceptedInvitation {
            workspace_id: accepted.workspace_id,
            container_id: accepted.container_id,
            container_kind,
        }))
    }

    /// Whether what a guest was let onto is a board or a wiki, which says the address
    /// they go to; asked of the organization this handle is of. One gone meanwhile has
    /// no kind, and neither has one that could not be read: the board's address tells
    /// them so.
    pub(super) async fn kind_of_admitted(
        &self,
        container_id: Option<&str>,
    ) -> Option<ContainerKind> {
        let container_id = container_id?;
        match self.store().get_container(container_id).await {
            Ok(container) => Some(container.kind),
            Err(error) if error.is_not_found() => None,
            Err(error) => {
                tracing::warn!(
                    %container_id,
                    %error,
                    "the kind of the board or wiki somebody was let onto was not read"
                );
                None
            }
        }
    }

    /// One attempt of [`App::accept_invitation`]: settled, or to be tried again for a
    /// maker who changed meanwhile. A new account is hashed once, into `new_user`.
    async fn try_accepting(
        &self,
        token: &str,
        accepting: Accepting<'_>,
        new_user: &mut Option<User>,
    ) -> Result<Tried, AppError> {
        let replacing = match accepting {
            Accepting::NewAccount { browser, .. } => self.replaced_by(browser),
            Accepting::SignedIn(_) => None,
        };
        let replacing = replacing.as_deref();
        let usable = self.usable_invitation(token, replacing);
        let Some((app, invitation, maker)) = usable.await? else {
            return Err(AppError::invitation_unavailable());
        };
        let inviter = maker.inviter(&invitation.created_by);
        let digest = token_digest(token);
        let now = now_millis();
        let mut waiting = None;
        // Only now, for a link that is there, is a password hashed, and once.
        let acceptance = match accepting {
            Accepting::SignedIn(asker) => {
                // Who holds the link knows the organization it leads into: a session
                // that organization does not take is told so, and let in nowhere.
                app.require_session_opens_organization(asker).await?;
                let acceptor = Acceptor::Account(asker.user_id);
                app.store()
                    .accept_invitation(&digest, acceptor, now, inviter)
                    .await
            }
            Accepting::NewAccount {
                account, language, ..
            } => {
                let user = match new_user {
                    Some(user) => user,
                    empty => empty.insert(app.new_account(account, replacing).await?),
                };
                if self.verifies_email() && !mailed_to(&invitation, &user.email) {
                    let browser = SignupMarker::new();
                    let signup = app
                        .signup_by_link(user, &invitation.id, language, &browser, now)
                        .await?;
                    let reserved = app
                        .store()
                        .reserve_invitation(&digest, &signup, replacing, now, inviter)
                        .await;
                    waiting = Some((signup, browser));
                    reserved
                } else {
                    let acceptor = match replacing {
                        Some(browser) => Acceptor::Replacing { user, browser },
                        None => Acceptor::NewAccount(user),
                    };
                    app.store()
                        .accept_invitation(&digest, acceptor, now, inviter)
                        .await
                }
            }
        }
        .map_err(|error| match error {
            // Used up after all, by the time the acceptance was written.
            StoreError::Conflict(Taken::LinkUse) => AppError::invitation_unavailable(),
            error @ StoreError::Conflict(_) => account_error(error),
            error => AppError::from(error).context("unable to accept an invitation"),
        })?;
        match acceptance {
            Acceptance::Accepted(accepted) => {
                if waiting.is_none() {
                    app.collab()
                        .recheck_access(
                            &app,
                            AccessScope::workspace(&accepted.workspace_id)
                                .of_user(&accepted.user_id),
                        )
                        .await;
                }
                Ok(Tried::Settled(Box::new(Settled {
                    app,
                    accepted,
                    waiting,
                })))
            }
            // Ended, used up or revoked since it was read.
            Acceptance::Unavailable => Err(AppError::invitation_unavailable()),
            Acceptance::InviterChanged => Ok(Tried::Again),
        }
    }

    /// Shows who came in by a guest's link to whoever is on its board.
    pub(super) async fn announce_admission(&self, accepted: &Accepted) {
        let (true, Some(container_id)) = (accepted.used, &accepted.container_id) else {
            return;
        };
        match self
            .store()
            .get_member_for_container(container_id, &accepted.user_id)
            .await
        {
            Ok(member) => {
                self.broadcast_member_change_in_background(accepted.workspace_id.clone(), member);
            }
            Err(error) => tracing::warn!(
                %error,
                container_id,
                "a guest who came in was not announced to the board"
            ),
        }
    }

    /// The maker of the link `invitation_id` of this application's organization, as
    /// they are now, if the link is there and they may still make it: what a
    /// registration made by the link is confirmed against.
    pub(super) async fn maker_of_link(
        &self,
        invitation_id: &str,
    ) -> Result<Option<(Invitation, Maker)>, AppError> {
        let Some(invitation) = self.store().invitation_by_id(invitation_id).await? else {
            return Ok(None);
        };
        Ok(self
            .inviter_who_may_still_invite(&invitation)
            .await?
            .map(|maker| (invitation, maker)))
    }
}

/// Whether `invitation` was mailed to the address `email`, which its mailbox proves: a
/// link made for one recipient of a mailed guest link.
fn mailed_to(invitation: &Invitation, email: &str) -> bool {
    // Addresses are ASCII (`fvoci_model::is_email_valid`), and folded as `lower()` folds.
    invitation
        .recipient
        .as_deref()
        .is_some_and(|recipient| recipient.eq_ignore_ascii_case(email))
}

/// Where an acceptance let somebody in, or that it waits for an address.
#[derive(Debug, Clone)]
pub enum Admission {
    /// Somebody was let in, here.
    LetIn(AcceptedInvitation),
    /// A registration waits for its address to be confirmed by the link mailed to it:
    /// nobody is let in until then. The marker is for the cookie of the browser it was
    /// made from.
    Waiting(SignupMarker),
}

/// An acceptance that was written.
struct Settled {
    /// The application behind the wall of the link's organization.
    app: App,
    accepted: Accepted,
    /// The registration that waits for its address, where a new account waits, and
    /// the marker of the browser it was made from.
    waiting: Option<(Signup, SignupMarker)>,
}

/// What an attempt to accept came to.
enum Tried {
    /// Accepted, by an account or by a registration that waits.
    Settled(Box<Settled>),
    /// The maker of the link changed meanwhile.
    Again,
}

/// Whether the user sees and revokes every link into a workspace, and not their own
/// alone: who invites its members.
const fn sees_every_link(actor: Actor) -> bool {
    permissions::allows_in_workspace(actor, WorkspacePermission::InviteMembers)
}
