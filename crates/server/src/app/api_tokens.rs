//! Personal API tokens as their owner makes, lists and takes them back, and as the
//! administrators of an organization see and take back those that reach it.

use std::net::IpAddr;

use fvoci_model::{
    API_TOKEN_DEFAULT_DAYS, API_TOKEN_MAX_DAYS, API_TOKEN_MAX_LIVE, API_TOKEN_NAME_MAX_CHARS,
    ApiToken, IssuedApiToken, MemberApiToken, NewApiToken, OrgKind, RefusalReason, Refusals,
    Session, new_id,
};
use fvoci_store::{GrantedWorkspace, Issuance, TokenToIssue};

use super::auth::WrongPassword;
use super::{App, sso};
use crate::auth::api_token::{DAY_MILLIS, make_token};
use crate::auth::now_millis;
use crate::error::AppError;

/// The most workspaces one token names.
pub(crate) const MAX_TOKEN_WORKSPACES: usize = 50;

/// The longest a token lasts that reaches an organization that takes only sessions
/// made through its provider, in days: what such an organization asks of who signs in
/// through it is asked again that often.
pub(crate) const SSO_TOKEN_MAX_DAYS: u32 = API_TOKEN_DEFAULT_DAYS;

/// A workspace a new token is to reach, as its maker was let into it.
struct Granted {
    workspace_id: String,
    org_id: String,
    /// Whether its organization takes only sessions made through its provider.
    sso_required: bool,
}

impl App {
    /// Makes a personal API token for the session's user, as `new` asks: told once, in
    /// the answer, and stored by its digest. It reaches the workspaces it names, each
    /// one the user works in and the session opens, and the user's own space if it asks
    /// for that and gives the account's password (if it has one).
    ///
    /// A workspace of an organization that takes only sessions made through its
    /// provider is given only where the organization lets its workspaces be given to
    /// tokens, and the token lasts no longer than `SSO_TOKEN_MAX_DAYS`.
    ///
    /// # Errors
    ///
    /// Returns 400 naming the field for a name, a number of days or a list of
    /// workspaces that is none, or that names the own space, and with the reason
    /// `too_many_api_tokens` for an account that holds as many tokens as it may; 404
    /// for a workspace the user does not work in; 401 for one the session does not open
    /// (`sso::require_session_opens`), and for a session that ended meanwhile; 403 with
    /// the reason `api_tokens_disallowed`; the errors of confirming the password; and
    /// the store's error.
    pub async fn create_api_token(
        &self,
        session: &Session,
        new: NewApiToken,
        address: Option<IpAddr>,
    ) -> Result<IssuedApiToken, AppError> {
        let name = checked_name(&new.name)?;
        let mut workspace_ids = new.workspace_ids.clone();
        workspace_ids.sort();
        workspace_ids.dedup();
        if workspace_ids.len() > MAX_TOKEN_WORKSPACES {
            return Err(refused(
                "workspaceIds",
                RefusalReason::TooLong,
                format_args!("a token names at most {MAX_TOKEN_WORKSPACES} workspaces"),
            ));
        }
        if workspace_ids.is_empty() && !new.allow_personal_space {
            return Err(refused(
                "workspaceIds",
                RefusalReason::Required,
                "a token reaches a workspace, or the own space",
            ));
        }
        let mut granted = Vec::with_capacity(workspace_ids.len());
        for workspace_id in &workspace_ids {
            granted.push(self.granted(session, workspace_id).await?);
        }
        if new.allow_personal_space {
            self.give_own_space(session, new.password.as_deref(), address)
                .await?;
        }
        let days = checked_days(new.expires_in_days)?;
        let days = if granted.iter().any(|granted| granted.sso_required) {
            days.min(SSO_TOKEN_MAX_DAYS)
        } else {
            days
        };
        self.issue(session, &new, &name, &granted, days).await
    }

    /// Stores the token `new` asks for, named `name`, reaching `granted` and lasting
    /// `days`, as checked: the token itself is told in the answer alone.
    async fn issue(
        &self,
        session: &Session,
        new: &NewApiToken,
        name: &str,
        granted: &[Granted],
        days: u32,
    ) -> Result<IssuedApiToken, AppError> {
        let now = now_millis();
        let made = make_token();
        let id = new_id();
        let workspaces: Vec<GrantedWorkspace<'_>> = granted
            .iter()
            .map(|granted| GrantedWorkspace {
                workspace_id: &granted.workspace_id,
                org_id: &granted.org_id,
            })
            .collect();
        // The store reads the provider the session was made through from the session,
        // and stores the token with it, or refuses it (`Issuance::ProviderChanged`).
        let issuance = self
            .store()
            .issue_api_token(&TokenToIssue {
                id: &id,
                user_id: &session.user_id,
                session_id: &session.id,
                name,
                token_prefix: &made.prefix,
                token_digest: &made.digest,
                scope: new.scope,
                workspaces: &workspaces,
                allow_personal_space: new.allow_personal_space,
                notify_self: new.notify_self,
                created_at: now,
                expires_at: now.saturating_add(i64::from(days) * DAY_MILLIS),
            })
            .await?;
        let api_token = match issuance {
            Issuance::Issued(api_token) => api_token,
            Issuance::NoLongerWorksIn(_) => {
                return Err(AppError::not_found("workspace").with_field("workspaceIds"));
            }
            Issuance::SessionEnded => {
                return Err(AppError::unauthorized("the session has ended"));
            }
            Issuance::ProviderChanged => return Err(AppError::provider_changed()),
            Issuance::TooMany => return Err(AppError::too_many_api_tokens(API_TOKEN_MAX_LIVE)),
        };
        tracing::info!(
            user_id = %session.user_id,
            token_id = %api_token.id,
            scope = %api_token.scope,
            "a personal API token was made"
        );
        Ok(IssuedApiToken {
            token: made.token,
            api_token,
        })
    }

    /// The personal API tokens of the user that are not taken back, the newest first.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn api_tokens_of_user(&self, user_id: &str) -> Result<Vec<ApiToken>, AppError> {
        Ok(self.store().api_tokens_of_user(user_id).await?)
    }

    /// Takes back a personal API token of the user: from then on it opens nothing.
    ///
    /// # Errors
    ///
    /// Returns 404 for a token that is not theirs or was taken back, and the store's
    /// error.
    pub async fn revoke_api_token(&self, user_id: &str, token_id: &str) -> Result<(), AppError> {
        if !fvoci_model::is_id(token_id)
            || !self
                .store()
                .revoke_api_token(user_id, token_id, now_millis())
                .await?
        {
            return Err(AppError::not_found("token"));
        }
        tracing::info!(user_id, token_id, "a personal API token was taken back");
        Ok(())
    }

    /// The personal API tokens that reach a workspace of this handle's organization
    /// and open anything now, with those of their workspaces that are its: for its
    /// administrators, who see nothing of what a token opens elsewhere.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn api_tokens_of_organization(&self) -> Result<Vec<MemberApiToken>, AppError> {
        Ok(self
            .store()
            .api_tokens_of_organization(now_millis())
            .await?)
    }

    /// Takes a personal API token that reaches a workspace of this handle's
    /// organization out of it, for its administrator `actor_id`: it reaches none of
    /// the organization's workspaces from then on, and is taken back if it reaches
    /// nothing else (`Store::revoke_api_token_of_organization`). The audit log of the
    /// organization records it.
    ///
    /// # Errors
    ///
    /// Returns 404 for a token that reaches none of its workspaces or was taken back,
    /// and the store's error.
    pub async fn revoke_api_token_of_organization(
        &self,
        actor_id: &str,
        token_id: &str,
    ) -> Result<(), AppError> {
        if !fvoci_model::is_id(token_id)
            || !self
                .store()
                .revoke_api_token_of_organization(actor_id, token_id, now_millis())
                .await?
        {
            return Err(AppError::not_found("token"));
        }
        tracing::info!(
            actor_id,
            token_id,
            org_id = self.store().tenant().org_id().unwrap_or_default(),
            "an administrator took a personal API token out of the organization"
        );
        Ok(())
    }

    /// A workspace the session's user works in and the session opens, to be given to
    /// a token.
    async fn granted(&self, session: &Session, workspace_id: &str) -> Result<Granted, AppError> {
        let not_found = || AppError::not_found("workspace").with_field("workspaceIds");
        let access = self
            .store()
            .resolve_workspace(&session.user_id, workspace_id)
            .await?
            .filter(fvoci_store::WorkspaceAccess::works_there)
            .ok_or_else(not_found)?;
        if access.kind == OrgKind::Personal {
            return Err(refused(
                "workspaceIds",
                RefusalReason::Invalid,
                "the own space is given by allowPersonalSpace",
            ));
        }
        sso::require_session_opens(session.into(), &access.org_id, &access.takes)?;
        let sso_required = self
            .require_tokens_given(&access)
            .await
            .map_err(|refused| refused.with_field("workspaceIds"))?;
        Ok(Granted {
            workspace_id: workspace_id.to_owned(),
            org_id: access.org_id,
            sso_required,
        })
    }

    /// Checks that the session's user may give their own space to a token: with the
    /// account's password, if it has one.
    async fn give_own_space(
        &self,
        session: &Session,
        password: Option<&str>,
        address: Option<IpAddr>,
    ) -> Result<(), AppError> {
        let user = self.store().get_user_by_id(&session.user_id).await?;
        if user.password_hash.is_empty() {
            return Ok(());
        }
        let Some(password) = password.filter(|password| !password.is_empty()) else {
            return Err(refused(
                "password",
                RefusalReason::Required,
                "the own space is given with the account's password",
            ));
        };
        self.confirm_password(
            &user,
            password,
            WrongPassword {
                words: "the password is wrong",
                field: "password",
            },
            address,
        )
        .await
    }
}

/// The name of a new token, trimmed: 1 to [`API_TOKEN_NAME_MAX_CHARS`] characters, no
/// control character.
fn checked_name(name: &str) -> Result<String, AppError> {
    let name = name.trim();
    let chars = name.chars().count();
    if chars == 0 {
        return Err(refused(
            "name",
            RefusalReason::Required,
            "a token has a name",
        ));
    }
    if chars > API_TOKEN_NAME_MAX_CHARS {
        return Err(refused(
            "name",
            RefusalReason::TooLong,
            format_args!("a token's name has at most {API_TOKEN_NAME_MAX_CHARS} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(refused(
            "name",
            RefusalReason::InvalidCharacters,
            "a token's name has no control character",
        ));
    }
    Ok(name.to_owned())
}

/// How many days a new token lasts: as asked, 1 to [`API_TOKEN_MAX_DAYS`], or
/// [`API_TOKEN_DEFAULT_DAYS`].
fn checked_days(asked: Option<u32>) -> Result<u32, AppError> {
    match asked {
        None => Ok(API_TOKEN_DEFAULT_DAYS),
        Some(days @ 1..=API_TOKEN_MAX_DAYS) => Ok(days),
        Some(_) => Err(refused(
            "expiresInDays",
            RefusalReason::Invalid,
            format_args!("a token lasts 1 to {API_TOKEN_MAX_DAYS} days"),
        )),
    }
}

fn refused(field: &str, reason: RefusalReason, detail: impl std::fmt::Display) -> AppError {
    let mut refusals = Refusals::new();
    refusals.refuse(field, reason, detail.to_string());
    refusals.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_and_held_to_its_length() {
        assert_eq!(checked_name("  agent  ").expect("a name"), "agent");
        assert!(checked_name("   ").is_err());
        assert!(checked_name(&"x".repeat(API_TOKEN_NAME_MAX_CHARS)).is_ok());
        assert!(checked_name(&"x".repeat(API_TOKEN_NAME_MAX_CHARS + 1)).is_err());
        assert!(checked_name("a\nb").is_err());
    }

    #[test]
    fn a_token_lasts_ninety_days_unless_asked_otherwise_within_a_year() {
        assert_eq!(checked_days(None).expect("days"), 90);
        assert_eq!(checked_days(Some(1)).expect("days"), 1);
        assert_eq!(checked_days(Some(365)).expect("days"), 365);
        assert!(checked_days(Some(0)).is_err());
        assert!(checked_days(Some(366)).is_err());
    }
}
