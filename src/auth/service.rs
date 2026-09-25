use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::auth::password::{hash_password, Keyring};
use crate::auth::session::SessionUser;
use crate::auth::token::hash_token;
use crate::auth::token::{new_token, SESSION_TTL_SECS};
use crate::db::identity::{
    authenticate_password, count_users, find_live_session, live_to_session_user,
    maybe_slide_session, new_setup_input, revoke_session, setup_first_owner, update_profile,
    ProfilePatch, SetupFirstOwnerResult, SetupSessionParams,
};
use crate::db::mfa::{issue_session_or_challenge, IssueOptions, Issued};
use crate::db::Db;

pub struct AuthService {
    pub db: Db,
    pub password_keys: Keyring,
}

pub struct SetupInstanceInput {
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub client_ip: Option<String>,
}

impl AuthService {
    pub async fn setup_needed(&self) -> Result<bool, sqlx::Error> {
        Ok(count_users(&self.db.pool).await? == 0)
    }

    pub async fn setup_instance(
        &self,
        password: String,
        input: SetupInstanceInput,
    ) -> Result<Result<(Uuid, Uuid, String), SetupError>, sqlx::Error> {
        if count_users(&self.db.pool).await? > 0 {
            return Ok(Err(SetupError::Closed));
        }

        let password_hash = hash_password(&password, &self.password_keys)
            .await
            .map_err(sqlx::Error::Protocol)?;

        let token = new_token();
        let expires_at = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
        let input = new_setup_input(SetupSessionParams {
            email: input.email,
            password_hash,
            given_name: input.given_name,
            family_name: input.family_name,
            workspace_slug: input.workspace_slug,
            workspace_name: input.workspace_name,
            token_hash: token.hash,
            expires_at,
            client_ip: input.client_ip,
        });
        let user_id = input.user_id;
        let workspace_id = input.workspace_id;
        let session_token = token.token;

        match setup_first_owner(&self.db.pool, input).await? {
            SetupFirstOwnerResult::Created => Ok(Ok((user_id, workspace_id, session_token))),
            SetupFirstOwnerResult::Closed => Ok(Err(SetupError::Closed)),
            SetupFirstOwnerResult::SlugTaken => Ok(Err(SetupError::SlugTaken)),
        }
    }

    /// Source `loginOrChallenge`: MFA is looked up only after the password
    /// verified, so failures look the same with or without MFA.
    pub async fn login(&self, email: &str, password: &str) -> Result<Option<Issued>, sqlx::Error> {
        let user_id =
            authenticate_password(&self.db.pool, email, password, &self.password_keys).await?;
        match user_id {
            Some(user_id) => {
                issue_session_or_challenge(
                    &self.db.pool,
                    user_id,
                    "password",
                    IssueOptions::default(),
                )
                .await
            }
            None => Ok(None),
        }
    }

    pub async fn session_user(&self, token: &str) -> Result<Option<SessionUser>, sqlx::Error> {
        let token_hash = hash_token(token);
        let live = find_live_session(&self.db.pool, &token_hash).await?;
        if let Some(live) = live {
            let expires_at =
                maybe_slide_session(&self.db.pool, live.session_id, live.expires_at).await?;
            let refreshed = if expires_at != live.expires_at {
                find_live_session(&self.db.pool, &token_hash).await?
            } else {
                Some(live)
            };
            return Ok(refreshed.as_ref().map(live_to_session_user));
        }
        Ok(None)
    }

    pub async fn logout(
        &self,
        token: &str,
        actor_user_id: Option<Uuid>,
    ) -> Result<(), sqlx::Error> {
        revoke_session(&self.db.pool, &hash_token(token), actor_user_id).await
    }

    pub async fn patch_profile(
        &self,
        token: &str,
        patch: ProfilePatch,
    ) -> Result<Result<SessionUser, ()>, sqlx::Error> {
        let token_hash = hash_token(token);
        let live = find_live_session(&self.db.pool, &token_hash).await?;
        let Some(live) = live else {
            return Ok(Err(()));
        };
        let user_id = live.user_id;

        let updated = update_profile(&self.db.pool, user_id, live.session_id, patch).await?;
        if !updated {
            return Ok(Err(()));
        }

        let refreshed = find_live_session(&self.db.pool, &token_hash).await?;
        match refreshed {
            Some(live) => Ok(Ok(live_to_session_user(&live))),
            None => Ok(Err(())),
        }
    }
}

#[derive(Debug)]
pub enum SetupError {
    Closed,
    SlugTaken,
}
