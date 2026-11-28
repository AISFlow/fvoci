//! Workspaces: which a user is a member of and the way into one, and what is told in
//! each of a user's. And the way into an organization: for its administrators, for a
//! member who asks to leave it, and for a sign-in through its provider.

use std::collections::HashSet;

use fvoci_model::{
    Container, ContainerKind, ContainerKinds, OrgKind, OrgRole, Session, UserWorkspace, Workspace,
    WorkspaceRole,
};
use std::time::Instant;

use fvoci_model::PublicContainers;
use fvoci_store::{Making, SessionTaken, Tenant, WorkspaceAccess};

use super::api_tokens::SSO_TOKEN_MAX_DAYS;
use super::{App, Asker, as_managed, sso};
use crate::auth::api_token::{self, Delegation};
use crate::auth::now_millis;
use crate::collab::AccessScope;
use crate::error::{AppError, OrMissing, Reason};

/// What an agent reaches of a workspace by a token: the kinds of container named here,
/// never a diary, which is its owner's alone. A kind that comes later is reached once
/// it is named.
pub(crate) const AGENT_KINDS: ContainerKinds = ContainerKinds::BOARDS
    .union(ContainerKinds::WIKIS)
    .union(ContainerKinds::COURSES);

/// A workspace as a member of it enters it: the application behind the wall between
/// organizations, and what the user is in the workspace and in its organization.
#[derive(Debug, Clone)]
pub struct Entered {
    /// The application, reaching the rows of the workspace's organization and no
    /// other. It is not narrowed to the workspace: the organization's other
    /// workspaces are in its reach.
    pub app: App,
    /// What the user is in the workspace.
    pub role: WorkspaceRole,
    /// What the user is in the workspace's organization; `None` for somebody who is in
    /// the workspace without belonging to the organization, and for a user's own space.
    pub org_role: Option<OrgRole>,
    /// What the workspace's organization is: one that people work in together, or the
    /// user's own space.
    pub kind: OrgKind,
}

impl App {
    /// The same server reaching the rows of the organization `org_id`, for an
    /// administrator of it; `None` for anybody else, and for an organization that is not
    /// there, not in use or the system's. And for a user's own space, its owner's too:
    /// nobody administers one (`Store::org_role_of_user`).
    ///
    /// # Errors
    ///
    /// Returns 401 with the reason `sso_required` for an administrator whose session
    /// does not open the organization (`sso::require_session_opens`), and the store's
    /// error if the membership or the provider cannot be read.
    pub async fn enter_org_as_admin(
        &self,
        session: &Session,
        org_id: &str,
    ) -> Result<Option<Self>, AppError> {
        let Ok(tenant) = Tenant::org(org_id) else {
            return Ok(None);
        };
        let actor = self.org_actor(&session.user_id, org_id).await?;
        if !actor.administers_the_organization() {
            return Ok(None);
        }
        let required = self
            .store()
            .required_provider_for(&session.user_id, org_id)
            .await?;
        let takes = SessionTaken::of_organization(required);
        sso::require_session_opens(session.into(), org_id, &takes)?;
        Ok(Some(self.scoped(tenant).with_org_kind(OrgKind::Shared)))
    }

    /// The same server reaching the rows of the organization `org_id`, for a member of
    /// it who asks to leave it of any session; `None` for anybody else, and for an
    /// organization that is not there, not in use, the system's, or a user's own space
    /// (`Store::org_role_of_user`).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read.
    pub(super) async fn enter_org_as_member(
        &self,
        user_id: &str,
        org_id: &str,
    ) -> Result<Option<Self>, AppError> {
        let Ok(tenant) = Tenant::org(org_id) else {
            return Ok(None);
        };
        if self.org_actor(user_id, org_id).await?.org.is_none() {
            return Ok(None);
        }
        Ok(Some(self.scoped(tenant).with_org_kind(OrgKind::Shared)))
    }

    /// The same server reaching the rows of the organization `org_id`, for a sign-in
    /// through its provider: the one a sign-in is started for, or the one a sign-in that
    /// comes back was started for. `None` for text that is no identifier.
    pub(crate) fn enter_org_for_sign_in(&self, org_id: &str) -> Option<Self> {
        Tenant::org(org_id).ok().map(|tenant| self.scoped(tenant))
    }

    /// The same server reaching the rows of the instance's own organization (the
    /// oldest), whose administrators follow who runs the server; `None` before the
    /// first sign-up.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the directory cannot be read.
    pub(crate) async fn instance_org(&self) -> Result<Option<Self>, AppError> {
        Ok(self
            .store()
            .instance_organization()
            .await?
            .and_then(|org_id| self.enter_org_for_sign_in(&org_id)))
    }
}

impl App {
    /// Lets a member into a workspace; `None` unless the session's user is a member of
    /// it, whether it exists or not.
    ///
    /// # Errors
    ///
    /// Returns 401 for a member whose session does not open the workspace: with the
    /// reason `sso_required` where its organization takes only sessions made through
    /// its provider, and `password_sign_in_required` for the user's own space, of an
    /// account that has a password, asked of a session made through a provider that
    /// has not confirmed the password (`sso::require_session_opens`); and the
    /// store's error if the membership cannot be read: that is no "not a member".
    pub async fn enter_workspace(
        &self,
        session: &Session,
        workspace_id: &str,
    ) -> Result<Option<Entered>, AppError> {
        self.enter_workspace_as(session.into(), workspace_id).await
    }

    /// As [`Self::enter_workspace`], for who is known by their user and the provider
    /// their session was made through: a request by its session, a websocket
    /// connection by what it kept of the session its handshake came with, an agent by
    /// a personal API token ([`Asker::delegation`]).
    ///
    /// A token is let in only where its owner works (a member, no guest) and that it
    /// names: a workspace of an organization it was given, or the owner's own space if
    /// it was given that. It is asked as the session that made it would be; and of an
    /// organization that takes only sessions made through its provider, whether that
    /// organization gives its workspaces to tokens. What it is let into reaches no
    /// diary ([`App::capped_to`]), and its requests there are counted with those of the
    /// organization's other tokens.
    ///
    /// # Errors
    ///
    /// As [`Self::enter_workspace`]; for a token, 403 with the reason
    /// `api_tokens_disallowed` where its organization gives its workspaces to none, and
    /// 429 with the reason `token_rate_limited` where the organization's tokens sent as
    /// many requests as they may this minute.
    pub async fn enter_workspace_as(
        &self,
        asker: Asker<'_>,
        workspace_id: &str,
    ) -> Result<Option<Entered>, AppError> {
        let Some(access) = self.admit_to_workspace(asker, workspace_id).await? else {
            return Ok(None);
        };
        let mut app = self
            .in_organization(&access.org_id)?
            .with_org_kind(access.kind);
        if let Some(delegation) = asker.delegation {
            self.authenticator()
                .token_limits()
                .admit_organization(&access.org_id, Instant::now())?;
            app = app.capped_to(AGENT_KINDS).writing_for(delegation);
        }
        Ok(Some(Entered {
            app,
            role: access.workspace_role,
            org_role: access.org_role,
            kind: access.kind,
        }))
    }

    /// Who is let into the workspace `workspace_id`, as [`Self::enter_workspace_as`] asks
    /// it and before it counts anything: the user's membership as the store has it,
    /// `None` unless they are a member (and, for a token, unless the token reaches it).
    ///
    /// # Errors
    ///
    /// As [`Self::enter_workspace_as`], but for the counts of the organization's tokens.
    async fn admit_to_workspace(
        &self,
        asker: Asker<'_>,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceAccess>, AppError> {
        let Some(access) = self
            .store()
            .resolve_workspace(asker.user_id, workspace_id)
            .await?
        else {
            return Ok(None);
        };
        if let Some(delegation) = asker.delegation
            && !self
                .delegation_reaches(delegation, workspace_id, &access)
                .await?
        {
            return Ok(None);
        }
        sso::require_session_opens(asker, &access.org_id, &access.takes)?;
        Ok(Some(access))
    }

    /// The workspaces the user of `asker` is a member of that it is let into, in the
    /// order of [`Store::workspaces_of_user`]: for a token, those it names and its
    /// owner still works in (and the owner's own space if it was given that), of the
    /// organizations that give their workspaces to tokens; for a session, those it
    /// opens. A workspace it is not let into is left out without a word of why, as a
    /// workspace that is not there; nothing is counted against the limits of tokens.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    ///
    /// [`Store::workspaces_of_user`]: fvoci_store::Store::workspaces_of_user
    pub async fn workspaces_let_in(
        &self,
        asker: Asker<'_>,
    ) -> Result<Vec<UserWorkspace>, AppError> {
        let workspaces = self
            .store()
            .workspaces_of_user(asker.user_id, asker.making)
            .await?;
        let mut let_in = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            match self.admit_to_workspace(asker, &workspace.id).await {
                Ok(Some(_)) => let_in.push(workspace),
                Ok(None) => {}
                Err(error)
                    if matches!(
                        error.reason(),
                        Some(
                            Reason::SsoRequired
                                | Reason::PasswordSignInRequired
                                | Reason::ApiTokensDisallowed
                        )
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(let_in)
    }

    /// Whether a token reaches a workspace its owner is a member of, as `access` was
    /// read: one its owner works in (no guest), and that it names, asked of the
    /// workspace's organization; or the owner's own space, if it was given that.
    ///
    /// # Errors
    ///
    /// Returns 403 with the reason `api_tokens_disallowed` for a workspace it names of
    /// an organization that takes only sessions made through its provider and gives
    /// its workspaces to no token; 401 with the reason `token_expired_for_organization`
    /// in such an organization for a token made more than `SSO_TOKEN_MAX_DAYS` before;
    /// and the store's error.
    async fn delegation_reaches(
        &self,
        delegation: &Delegation,
        workspace_id: &str,
        access: &WorkspaceAccess,
    ) -> Result<bool, AppError> {
        if !access.works_there() {
            return Ok(false);
        }
        if access.kind == OrgKind::Personal {
            return Ok(delegation.allow_personal_space);
        }
        let reach = self
            .in_organization(&access.org_id)?
            .store()
            .api_token_reach(&delegation.token_id, workspace_id)
            .await?;
        let Some(reach) = reach else {
            return Ok(false);
        };
        if matches!(access.takes, SessionTaken::Through(_)) {
            if !reach.allow_api_tokens {
                return Err(api_token::disallowed_by_organization());
            }
            // The token was made to last longer before the organization took only such
            // sessions, or reached none of its workspaces then.
            if delegation.ends_within(SSO_TOKEN_MAX_DAYS) <= now_millis() {
                return Err(api_token::past_its_days_there());
            }
        }
        Ok(true)
    }

    /// Refuses to give a token a workspace of `access`'s organization where it takes
    /// only sessions made through its provider and gives its workspaces to no token
    /// (`allowApiTokens`); an organization that takes any session gives them whatever.
    /// Returns whether it takes only such sessions.
    ///
    /// # Errors
    ///
    /// Returns 403 with the reason `api_tokens_disallowed`, and the store's error.
    pub(crate) async fn require_tokens_given(
        &self,
        access: &WorkspaceAccess,
    ) -> Result<bool, AppError> {
        if !matches!(access.takes, SessionTaken::Through(_)) {
            return Ok(false);
        }
        let organization = self
            .in_organization(&access.org_id)?
            .store()
            .organization()
            .await?;
        if organization.allow_api_tokens {
            Ok(true)
        } else {
            Err(api_token::disallowed_by_organization())
        }
    }

    /// [`Self::enter_workspace_as`], mapping session-gated refusals (`sso_required` and
    /// `password_sign_in_required`) to `Ok(None)` while propagating real store failures.
    pub(crate) async fn enter_workspace_quietly(
        &self,
        asker: Asker<'_>,
        workspace_id: &str,
    ) -> Result<Option<Entered>, AppError> {
        match self.enter_workspace_as(asker, workspace_id).await {
            Ok(entered) => Ok(entered),
            Err(error)
                if matches!(
                    error.reason(),
                    Some(Reason::SsoRequired | Reason::PasswordSignInRequired)
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// The application as the holder of a read token reaches what it opens: behind the
    /// wall of its own organization, whichever the request came in for, and reaching
    /// its kind alone (boards for a board's token, wikis for a wiki's), which the
    /// routes of the link ask ([`App::kinds`]). `None` if the token opens nothing, or
    /// is empty.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the board's sharing cannot be read: that is no
    /// "no", and answered as one it would tell a reader with a good link that the link
    /// is dead.
    pub async fn enter_shared_container(
        &self,
        container_id: &str,
        read_token: &str,
    ) -> Result<Option<Self>, AppError> {
        if read_token.is_empty() {
            return Ok(None);
        }
        let shared = self
            .authenticator()
            .shared_by_read_token(container_id, read_token)
            .await?;
        shared
            .map(|shared| {
                Ok(self
                    .in_organization(&shared.org_id)?
                    .reaching(shared.kind.into()))
            })
            .transpose()
    }

    /// Tells, in each workspace the user is a member of in any organization, whoever
    /// follows it and sees the user that their name changed.
    pub(super) fn broadcast_person_change_in_background(&self, user_id: String) {
        let app = self.clone();
        self.spawn_background(async move {
            // Which of them a session opens is not asked: the name changed in all.
            let every = Making::with_password();
            let workspaces = match app.store().workspaces_of_user(&user_id, every).await {
                Ok(workspaces) => workspaces,
                Err(error) => {
                    tracing::error!(%error, user_id = %user_id, "reading the workspaces of a renamed user failed");
                    return;
                }
            };
            for workspace in workspaces {
                match app.in_organization(&workspace.org_id) {
                    Ok(scoped) => {
                        scoped
                            .ws()
                            .broadcast_person_change(&workspace.id, &user_id)
                            .await;
                    }
                    Err(error) => tracing::error!(
                        %error,
                        user_id = %user_id,
                        org_id = %workspace.org_id,
                        "a renamed user's organization"
                    ),
                }
            }
        });
    }

    /// The application behind the wall of the organization the store named.
    pub(crate) fn in_organization(&self, org_id: &str) -> Result<Self, AppError> {
        // What the store answered is an identifier; one that is none is the server's
        // fault, not the request's.
        let tenant = Tenant::org(org_id).map_err(|error| {
            AppError::from(anyhow::Error::from(error))
                .context("the organization of a row as the store has it")
        })?;
        Ok(self.scoped(tenant))
    }

    /// [`App::kind_of_admitted`] asked behind the wall of `org_id`, the organization the
    /// store said an account just confirmed is of.
    ///
    /// # Errors
    ///
    /// Returns an internal error if `org_id` is no identifier.
    pub(super) async fn kind_of_admitted_in(
        &self,
        org_id: &str,
        container_id: Option<&str>,
    ) -> Result<Option<ContainerKind>, AppError> {
        let within = self.in_organization(org_id)?;
        Ok(within.kind_of_admitted(container_id).await)
    }

    /// [`App::welcome_member`] behind the wall of `org_id`, the organization the store
    /// said an account just confirmed came into as a member.
    pub(super) async fn welcome_member_in(
        &self,
        org_id: &str,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
    ) {
        match self.in_organization(org_id) {
            Ok(within) => {
                within
                    .welcome_member(user_id, workspace_id, accept_language)
                    .await;
            }
            Err(error) => {
                tracing::warn!(user_id, %error, "a new member was not given their welcome board");
            }
        }
    }

    /// [`App::welcome_workspace`] behind the wall of `org_id`, for a newly founded
    /// shared organization workspace or personal space.
    pub(super) async fn welcome_workspace_in(
        &self,
        org_id: &str,
        user_id: &str,
        workspace_id: &str,
        org_kind: OrgKind,
        accept_language: Option<&str>,
    ) {
        match self.in_organization(org_id) {
            Ok(within) => {
                within
                    .welcome_workspace(org_id, user_id, workspace_id, org_kind, accept_language)
                    .await;
            }
            Err(error) => {
                tracing::warn!(
                    user_id,
                    workspace_id,
                    %error,
                    "a workspace was not given its welcome board"
                );
            }
        }
    }

    /// Whether the user works in the workspace: a member of the workspace who is no guest.
    /// This is what the routes that name a workspace in their body or query, and not under
    /// `/w/{workspaceId}`, ask before anything else; for anybody else the workspace is not
    /// there. The routes under a workspace ask it of their path (`InWorkspace::require_no_guest`).
    ///
    /// A guest of a workspace is on the boards they were invited to and reaches those
    /// by their board; what a member of the workspace may do (see its boards, make one,
    /// join an open one) a guest may not, until the roles of a workspace say more
    /// (`WorkspaceAccess::works_there`).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read: that is no "no".
    pub async fn works_in_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<bool, AppError> {
        Ok(self.actor(user_id, workspace_id).await?.works_there())
    }

    /// The workspaces the session's user is a member of, in every organization: by
    /// organization, the oldest first, and in each the oldest workspace first. One the
    /// session does not open says so (`UserWorkspace::sso_required`) and is listed all
    /// the same: the user is told where to sign in through a provider.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn workspaces_of_user(
        &self,
        session: &Session,
    ) -> Result<Vec<UserWorkspace>, AppError> {
        Ok(self
            .store()
            .workspaces_of_user(&session.user_id, session.into())
            .await?)
    }

    /// The boards of a workspace as the user lists them, of the kinds the handle reaches
    /// ([`App::reaching`]): an administrator of the workspace every one of it, those they
    /// do not read without their description and properties ([`as_managed`]); whoever
    /// else those they are a member of and the open ones
    /// ([`App::readable_workspace_containers`]). Templates are left out. Oldest first.
    ///
    /// The handle is the one a member entered the workspace with
    /// ([`App::enter_workspace`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn workspace_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
        role: WorkspaceRole,
    ) -> Result<Vec<Container>, AppError> {
        let readable = self
            .readable_workspace_containers(user_id, workspace_id, role)
            .await?;
        if role != WorkspaceRole::Admin {
            return Ok(readable);
        }
        // What the administrator does not read is listed as who only manages it sees it.
        let read: HashSet<String> = readable.into_iter().map(|container| container.id).collect();
        let every = self
            .store()
            .get_containers_of_workspace(workspace_id, self.kinds())
            .await?;
        Ok(every
            .into_iter()
            .map(|container| {
                if read.contains(&container.id) {
                    container
                } else {
                    as_managed(container)
                }
            })
            .collect())
    }

    /// The boards of a workspace that the user is a member of and, unless the user is a
    /// guest of the workspace, its open boards, of the kinds the handle reaches: what an
    /// archive of the workspace holds for the user. Templates are left out. Oldest first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn readable_workspace_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
        role: WorkspaceRole,
    ) -> Result<Vec<Container>, AppError> {
        let public_containers = match role {
            WorkspaceRole::Guest => PublicContainers::Exclude,
            WorkspaceRole::Admin | WorkspaceRole::Member => PublicContainers::Include,
        };
        self.containers_for_user_and_workspace(user_id, workspace_id, public_containers)
            .await
    }

    /// Returns the workspace with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns a 404 if there is no such workspace, and the store's error if the query fails.
    pub async fn workspace(&self, workspace_id: &str) -> Result<Workspace, AppError> {
        self.store()
            .get_workspace(workspace_id)
            .await
            .or_missing("workspace")
    }

    /// Returns every workspace, oldest first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn workspaces(&self) -> Result<Vec<Workspace>, AppError> {
        Ok(self.store().get_workspaces().await?)
    }

    /// Puts the connections of `session`, which gave its account the first password
    /// and has not shown it, out of the account's own space: the space takes a session
    /// that has shown the password from here on. What they follow of it ends and its
    /// rooms let them go; they stay open for what the session still opens, and for the
    /// space once the session gives the password (`App::confirm_session_password`).
    /// If the space cannot be read the connections are closed instead, and come back
    /// to be asked.
    pub(super) async fn shut_own_space_to(&self, session: &Session) {
        let shut = async {
            let workspaces = self
                .store()
                .workspaces_of_user(&session.user_id, session.into())
                .await?;
            for own in workspaces
                .iter()
                .filter(|workspace| workspace.kind == OrgKind::Personal)
            {
                let space = self.in_organization(&own.org_id)?;
                space
                    .ws()
                    .end_subscriptions_without_password(&own.id, &session.user_id);
                let scope = AccessScope::workspace(&own.id).of_user(&session.user_id);
                space.collab().recheck_access(&space, scope).await;
            }
            Ok::<(), AppError>(())
        };
        if let Err(error) = shut.await {
            tracing::warn!(%error, user_id = %session.user_id, "the own space was not shut to the session that set its password; its connections are closed");
            self.ws().close_session_unread(&session.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use fvoci_model::ContainerKinds;

    use super::AGENT_KINDS;

    /// An agent reaches every kind but the diary: a kind that comes later is named
    /// for it, or left out, by a change of this test too.
    #[test]
    fn an_agent_reaches_every_kind_of_container_but_a_diary() {
        assert!(AGENT_KINDS.intersection(ContainerKinds::DIARIES).is_empty());
        assert_eq!(
            AGENT_KINDS.union(ContainerKinds::DIARIES),
            ContainerKinds::EVERY
        );
    }
}
