use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::auth::password::{hash_password, Keyring};
use crate::auth::session::SessionUser;
use crate::auth::token::hash_token;
use crate::auth::token::{new_token, SESSION_TTL_SECS};
use crate::db::identity::{
    authenticate_password, count_users, find_live_session, issue_session, live_to_session_user,
    maybe_slide_session, new_setup_input, revoke_session, setup_first_owner, update_profile,
    ProfilePatch, SetupFirstOwnerResult,
};
use crate::db::Db;

pub struct AuthService {
    pub db: Db,
    pub password_keys: Keyring,
}

impl AuthService {
    pub async fn setup_needed(&self) -> Result<bool, sqlx::Error> {
        Ok(count_users(&self.db.pool).await? == 0)
    }

    pub async fn setup_instance(
        &self,
        email: String,
        password: String,
        given_name: String,
        family_name: Option<String>,
        workspace_slug: String,
        workspace_name: String,
    ) -> Result<Result<(Uuid, Uuid, String), SetupError>, sqlx::Error> {
        if count_users(&self.db.pool).await? > 0 {
            return Ok(Err(SetupError::Closed));
        }

        let password_hash = hash_password(&password, &self.password_keys)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.into()))?;

        let token = new_token();
        let expires_at = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
        let input = new_setup_input(
            email,
            password_hash,
            given_name,
            family_name,
            workspace_slug,
            workspace_name,
            token.hash,
            expires_at,
        );
        let user_id = input.user_id;
        let workspace_id = input.workspace_id;
        let session_token = token.token;

        match setup_first_owner(&self.db.pool, input).await? {
            SetupFirstOwnerResult::Created => Ok(Ok((user_id, workspace_id, session_token))),
            SetupFirstOwnerResult::Closed => Ok(Err(SetupError::Closed)),
            SetupFirstOwnerResult::SlugTaken => Ok(Err(SetupError::SlugTaken)),
        }
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<Option<(Uuid, String)>, sqlx::Error> {
        let user_id =
            authenticate_password(&self.db.pool, email, password, &self.password_keys).await?;
        if let Some(user_id) = user_id {
            if let Some(token) = issue_session(&self.db.pool, user_id).await? {
                return Ok(Some((user_id, token)));
            }
        }
        Ok(None)
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

        let updated = update_profile(&self.db.pool, user_id, patch).await?;
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
