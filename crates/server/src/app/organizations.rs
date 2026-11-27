//! Starting an organization from an account that is there already, and leaving one at
//! the account's own asking.

use std::net::IpAddr;

use fvoci_model::{ContainerMember, ContainerRole, OrgKind, Session, UserWorkspace, WorkspaceRole};
use fvoci_store::{Departure, DepartureRefused, LeftFrom, RemovalRefused};

use super::App;
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};

impl App {
    /// Makes an organization that people work in together, and its first workspace,
    /// both under `name`, for the account of `session`, which administers both.
    /// Returns the workspace as the account is listed it. The caller has read the name
    /// as one is stored (`NewOrganization::stored_name`).
    ///
    /// # Errors
    ///
    /// Returns a permission error (reason `invitation_required`) on a server that
    /// takes one organization; 400 with the reason `too_many_organizations` and the
    /// limit for an account that made as many as one may ([`App::account_limits`]); a
    /// 429 if there are too many registrations from the address, with which this is
    /// counted; and the store's error if the organization cannot be stored.
    pub async fn start_organization(
        &self,
        session: &Session,
        name: &str,
        address: Option<IpAddr>,
    ) -> Result<UserWorkspace, AppError> {
        self.start_organization_with_language(session, name, address, None)
            .await
    }

    /// [`App::start_organization`], passing `accept_language` (`Accept-Language`) for
    /// creating the new organization's default-workspace welcome board.
    ///
    /// # Errors
    ///
    /// As [`App::start_organization`].
    pub async fn start_organization_with_language(
        &self,
        session: &Session,
        name: &str,
        address: Option<IpAddr>,
        accept_language: Option<&str>,
    ) -> Result<UserWorkspace, AppError> {
        // Where one organization is taken, the first user started it by registering.
        if !self.settings().takes_several_organizations() {
            return Err(AppError::invitation_required());
        }
        // An organization is what a registration makes, and is counted against where
        // it comes from as one is: a session is not a way around that count.
        let turn = self.admit_registration(address)?;
        let started = self
            .store()
            .start_organization(
                &session.user_id,
                name,
                self.account_limits().organizations_made,
            )
            .await?;
        drop(turn);
        tracing::info!(
            user_id = %session.user_id,
            org_id = %started.org_id,
            "an account started an organization"
        );
        self.welcome_workspace_in(
            &started.org_id,
            &session.user_id,
            &started.workspace_id,
            OrgKind::Shared,
            accept_language,
        )
        .await;
        Ok(UserWorkspace {
            id: started.workspace_id,
            org_id: started.org_id,
            kind: OrgKind::Shared,
            name: name.to_owned(),
            role: WorkspaceRole::Admin,
            // An organization that was just made has no provider to require.
            sso_required: false,
            password_required: false,
        })
    }

    /// Takes the account of `session` out of the organization `org_id` at its own
    /// asking ([`fvoci_store::Store::leave_organization`]), and then tells who is to be
    /// told (`App::announce_leaving`). Its sessions, its identities and what it wrote
    /// stay. Asked of any session: leaving an organization is the account's own, as
    /// deleting the account takes it out of one without asking its provider.
    ///
    /// # Errors
    ///
    /// Returns a 404 (`organization`) for an organization that is not there, is none
    /// that people work in together (an own space), or the account is no member of (a
    /// guest of its workspaces is none); a 409 with the reason `password_required`,
    /// `last_instance_admin`, `sole_org_admin` or `alone_in_organization` for what
    /// stands in the way, and nothing changed then; and the store's error.
    pub async fn leave_organization(
        &self,
        session: &Session,
        org_id: &str,
    ) -> Result<(), AppError> {
        let user_id = session.user_id.as_str();
        let Some(within) = self.enter_org_as_member(user_id, org_id).await? else {
            return Err(AppError::not_found("organization"));
        };
        within
            .collab()
            .narrowing_access(&within, &AccessScope::organization().of_user(user_id));
        match within.store().leave_organization(user_id).await? {
            Departure::Left(left) => {
                tracing::info!(org_id, user_id, "an account left an organization");
                within.after_leaving(org_id, user_id, &left).await;
                Ok(())
            }
            Departure::NotAMember => Err(AppError::not_found("organization")),
            Departure::Refused(refused) => Err(departure_refusal(refused)),
        }
    }

    /// What follows an account's leaving or being taken out of the handle's
    /// organization: what stays without an administrator and the boards that passed are
    /// logged, and who is to be told is ([`App::announce_leaving`]).
    pub(super) async fn after_leaving(&self, org_id: &str, user_id: &str, left: &LeftFrom) {
        for workspace_id in &left.workspaces_without_administrator {
            tracing::warn!(
                org_id,
                workspace_id,
                user_id,
                "a workspace has no administrator: the account that left the organization \
                 was its only one. An administrator of the organization can join it and \
                 name one"
            );
        }
        for container_id in &left.boards_without_admin {
            tracing::warn!(
                org_id,
                container_id,
                user_id,
                "a board has no admin: the account that left the organization was its only \
                 one, and its workspace has no other administrator to pass it to"
            );
        }
        for passed in &left.handed_over {
            tracing::info!(
                org_id,
                workspace_id = %passed.workspace_id,
                container_id = %passed.container_id,
                heir_id = %passed.heir_id,
                "a board passed to an administrator of its workspace as its admin left the \
                 organization"
            );
        }
        self.announce_leaving(user_id, left).await;
    }

    /// Tells of an account's leaving the handle's organization, as it is erased or of
    /// its own accord: its connections stop following each workspace it left
    /// (`workspaceUnsubscribed`), whose rooms ask about it again, as for a member taken
    /// out; each board that passed to somebody is announced to who is on it
    /// (`containerMemberChanged`), and its rooms ask about the heir, who may write there now.
    pub(super) async fn announce_leaving(&self, user_id: &str, left: &LeftFrom) {
        for workspace_id in &left.left {
            self.ws().workspace_role_changed(workspace_id, user_id);
        }
        self.collab()
            .recheck_access(self, AccessScope::organization().of_user(user_id))
            .await;
        for passed in &left.handed_over {
            let admin =
                ContainerMember::new(&passed.container_id, &passed.heir_id, ContainerRole::Admin);
            self.broadcast_member_change_in_background(passed.workspace_id.clone(), admin);
            self.collab()
                .recheck_access(
                    self,
                    AccessScope::container(&passed.container_id).of_user(&passed.heir_id),
                )
                .await;
        }
    }
}

/// The 409 of what stands in the way of an account's leaving an organization.
fn departure_refusal(refused: DepartureRefused) -> AppError {
    match refused {
        DepartureRefused::PasswordRequired => {
            AppError::conflict("set a password for the account before leaving the organization")
                .with_reason(Reason::PasswordRequired)
        }
        DepartureRefused::LastInstanceAdmin => removal_refusal(RemovalRefused::LastInstanceAdmin),
        DepartureRefused::SoleAdmin => removal_refusal(RemovalRefused::SoleAdmin),
        DepartureRefused::Alone => removal_refusal(RemovalRefused::Alone),
    }
}

/// The 409 of what stands in the way of taking a member out of an organization (or
/// leaving it, after checking for a password).
pub(super) fn removal_refusal(refused: RemovalRefused) -> AppError {
    let (detail, reason) = match refused {
        RemovalRefused::LastInstanceAdmin => (
            "the server must keep an administrator: make somebody else an administrator of \
             the organization first",
            Reason::LastInstanceAdmin,
        ),
        RemovalRefused::SoleAdmin => (
            "the organization must keep an administrator: make somebody else one first",
            Reason::SoleOrgAdmin,
        ),
        RemovalRefused::Alone => (
            "nobody else is in the organization: it would be left empty",
            Reason::AloneInOrganization,
        ),
    };
    AppError::conflict(detail).with_reason(reason)
}
