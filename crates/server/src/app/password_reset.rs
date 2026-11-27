//! Recovering native accounts with one-hour, single-use reset links.

use std::net::IpAddr;
use std::time::Instant;

use super::language::Language;
use super::mail::{Content, Letter, SecretLink, SecretPage};
use crate::auth::limiter::RetryAfter;

use super::App;
use crate::auth::now_millis;
use crate::auth::secret::{new_secret, token_digest};
use crate::error::AppError;
use crate::metrics::ResetMailDrop;

impl App {
    /// Creates a reset link for an operator to deliver through the administration socket,
    /// recorded as theirs in each organization of the user
    /// ([`fvoci_store::Store::create_password_reset_by_operator`]).
    ///
    /// An account that waits to be deleted gets one too: with it its person signs in and
    /// may take the deletion back.
    ///
    /// # Errors
    /// Returns not found for an unavailable account, or the storage error.
    pub async fn password_reset_link(&self, username: &str) -> Result<String, AppError> {
        let user = self.store().account_by_username(username).await?;
        let link = self.reset_link_for(&user.id, Asked::ByOperator).await?;
        Ok(link.into_string())
    }

    async fn reset_link_for(
        &self,
        user_id: &str,
        asked: Asked<'_>,
    ) -> Result<SecretLink, AppError> {
        let token = new_secret();
        let (digest, now) = (token_digest(&token), now_millis());
        let store = self.store();
        let stored = match asked {
            Asked::ByOperator => {
                store
                    .create_password_reset_by_operator(user_id, &digest, now)
                    .await?
            }
            Asked::ByMail { to } => {
                store
                    .create_password_reset(user_id, to, &digest, now)
                    .await?
            }
        };
        if !stored {
            return Err(AppError::not_found("account with a password"));
        }
        Ok(SecretLink::new(
            self.settings().server_url(),
            SecretPage::ResetPassword,
            &token,
        ))
    }

    /// Admits recovery identically for existing and absent accounts and returns unstarted work.
    /// The HTTP adapter releases this work only after handing off its response.
    ///
    /// Requests are limited by their address only, as logins are by theirs: a limit on
    /// the email would lock its owner out for whoever asks in their name. The hourly
    /// limit (`mail_reset_per_account_hour`) and queue place are taken in the
    /// background before looking up the account, and requests that mail nothing still
    /// take their turn of the pace (`Queued::pass`), so neither the answer nor the
    /// timing of later mail reveals whether an account with a password has that address.
    ///
    /// # Errors
    /// Returns 404 without mail, 400 for a malformed email, or 429 for too many
    /// requests from this address, independently of login limits.
    pub fn prepare_password_reset(
        &self,
        email: &str,
        language: &str,
        address: Option<IpAddr>,
    ) -> Result<impl Future<Output = ()> + Send + 'static + use<>, AppError> {
        if !self.config().mail_enabled() {
            return Err(AppError::not_found("route"));
        }
        let email = email.trim().to_lowercase();
        if !fvoci_model::is_email_valid(&email) {
            return Err(
                AppError::bad_request("a valid email address is required").with_field("email")
            );
        }
        // Counted against the address as it is admitted; nothing is counted at the email.
        let _request = self
            .inner
            .reset_limiter
            .admit_from_address(address, Instant::now())
            .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?;
        let app = self.clone();
        let requested = Language::written_in(language);
        Ok(async move {
            // The cause is the store's: what became of the message is told where it is sent.
            if let Err(error) = app.mail_password_reset(&email, requested).await {
                tracing::warn!(%error, "password recovery delivery failed");
            }
        })
    }

    async fn mail_password_reset(
        &self,
        email: &str,
        requested: Option<Language>,
    ) -> Result<(), AppError> {
        // Counted at the address, and in the queue, before anything is known of it:
        // neither the hourly limit nor the pace's next turn tells whether an account
        // with a password has this address.
        let queued = match self.inner.mail.queue_reset(email) {
            Ok(queued) => queued,
            Err(reason) => {
                match reason {
                    ResetMailDrop::AccountLimit => {
                        tracing::info!(
                            "password recovery mail dropped: the address's hourly limit"
                        );
                    }
                    ResetMailDrop::QueueFull => {
                        tracing::warn!("password recovery mail dropped: its queue is full");
                    }
                }
                self.metrics().password_reset_mail_dropped(reason);
                return Ok(());
            }
        };
        // An account that waits to be deleted is sent one as one in use is: the way back
        // is signing in, which needs a password its person may have forgotten.
        let user = match self.store().account_by_email(email).await {
            Ok(user) => user,
            Err(error) if error.is_not_found() => {
                // The turn of the pace a link would have taken is taken: how long the
                // next message to anybody waits tells nothing either.
                queued.pass(&self.inner.mail).await;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if user.password_hash.is_empty() {
            queued.pass(&self.inner.mail).await;
            return Ok(());
        }
        let settings = self.user_settings(&user.id).await?;
        let language =
            Language::first_of([Language::written_in(settings.language.as_str()), requested]);
        // The link is made once the message has its turn: its hour starts as it leaves.
        queued
            .deliver(
                &self.inner.mail,
                || async {
                    // Made only while the address the message goes to is still the
                    // account's: a change of it confirmed while the message waited
                    // ends the message here.
                    let asked = Asked::ByMail { to: &user.email };
                    let link = self.reset_link_for(&user.id, asked).await?;
                    Ok::<_, AppError>(Letter {
                        to: user.email.clone(),
                        content: Content::reset(&link, language),
                    })
                },
                // The link was made for this message, and holds until it is used or ends.
                || async { Ok(true) },
            )
            .await
            .map(|_| ())
            .or_else(|error| {
                // The account went, or its address changed, while the message waited.
                if error.is_not_found() {
                    Ok(())
                } else {
                    Err(error)
                }
            })
    }

    /// Sets the password proved by a reset token and closes every session and live
    /// connection. Who holds the token proved the mailbox, or was given the link by an
    /// operator, so the sign-in lock that others' guesses at the old password left on
    /// the account ends too, with the wait of this address at it.
    ///
    /// # Errors
    /// Returns 400 for an invalid password or unavailable token, 429 for too many
    /// attempts, or the storage error. An unavailable token never changes a password.
    pub async fn finish_password_reset(
        &self,
        token: &str,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<(), AppError> {
        let _attempt = self.admit_registration(address)?;
        fvoci_model::check_new_password(password)?;
        let hash = self.hash(password).await?;
        let fvoci_store::PasswordSet { user_id, ended } = self
            .store()
            .consume_password_reset(&token_digest(token), &hash, now_millis())
            .await?
            .ok_or_else(|| {
                AppError::bad_request("the reset link is invalid or expired").with_field("token")
            })?;
        self.ws().close(ended.into_scope());
        match self.store().account_by_id(&user_id).await {
            Ok(user) => self
                .login_limiter()
                .forget_account(&[&user.username, &user.email], address),
            // The password is set; an account deleted meanwhile has nothing to unlock.
            Err(error) if error.is_not_found() => {}
            // The password is set: what is left of the lock ends on its own.
            Err(error) => tracing::warn!(%error, "the account's sign-in lock was not ended"),
        }
        Ok(())
    }
}

/// Who asked for a reset link: the server's operator, whose act each organization of
/// the user records, or the person by mail, which is no one's act on them.
#[derive(Debug, Clone, Copy)]
enum Asked<'a> {
    ByOperator,
    /// To the address `to`, which was the account's when the mail was asked for.
    ByMail {
        to: &'a str,
    },
}
