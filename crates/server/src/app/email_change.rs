//! An account changes its email address: asked for with the account's password, and
//! made by the link mailed to the new address, or at once where the server sends no
//! mail and so confirms no address; and the link mailed to the address before gives it
//! back.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use futures_util::future::BoxFuture;
use fvoci_model::{Me, Session};
use fvoci_store::EmailReverted;

use super::App;
use super::auth::{WrongPassword, account_error};
use super::language::Language;
use super::mail::{Content, Letter, Queued, SecretLink, SecretPage};
use crate::auth::limiter::RetryAfter;
use crate::auth::now_millis;
use crate::auth::secret::{new_secret, token_digest};
use crate::error::{AppError, Reason};
use crate::metrics::ResetMailDrop;

/// How the password of a change of address is refused.
const WRONG: WrongPassword = WrongPassword {
    words: "the password is wrong",
    field: "password",
};

/// What a held lock on the digest of a link says when it is taken.
const DIGEST_LOCK: &str = "the link's digest is never held across a panic";

/// A change of address an account waits for, as its mail is written.
struct Waiting {
    user_id: String,
    username: String,
    /// The address that is to be confirmed.
    email: String,
}

impl App {
    /// The user of a session as their own account is answered to them: with the
    /// address it has, and the one it waits to have confirmed.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such user, and the store's error.
    pub async fn me(&self, user_id: &str) -> Result<Me, AppError> {
        let user = self.user(user_id).await?;
        let waits_for = self.store().email_change_of(user_id, now_millis()).await?;
        Ok(Me::of(user, waits_for))
    }

    /// Asks for the address of the account of `session` to become `email`, for whoever
    /// knows the account's `password`. Where the server sends mail, the account waits
    /// from then on, in place of what it waited for, and the returned work mails the
    /// link that makes the change to `email`: it is to run once the answer is handed
    /// off, so that the answer and its timing are the same whether or not somebody
    /// has the address. Where it sends none, the address is changed here, and the
    /// work is nothing.
    ///
    /// # Errors
    ///
    /// Returns 400 naming `email` for what is no address or is the account's already,
    /// 409 (`password_not_set`) for an account without a password, 429 for too many
    /// requests from `address`, 400 (`invalid_credentials`) for a wrong password or 429
    /// once too many were tried, 409 if the password was changed meanwhile, and, only
    /// where no mail is sent, 409 (`email_taken`) for an address that is another
    /// account's.
    pub async fn ask_email_change(
        &self,
        session: &Session,
        email: &str,
        password: &str,
        language: &str,
        address: Option<IpAddr>,
    ) -> Result<BoxFuture<'static, ()>, AppError> {
        let email = email.trim();
        if !fvoci_model::is_email_valid(email) {
            return Err(
                AppError::bad_request("a valid email address is required").with_field("email")
            );
        }
        let user = self.store().get_user_by_id(&session.user_id).await?;
        if user.password_hash.is_empty() {
            return Err(
                AppError::conflict("the account has no password to confirm this with")
                    .with_reason(Reason::PasswordNotSet),
            );
        }
        if email.to_lowercase() == user.email.to_lowercase() {
            return Err(
                AppError::bad_request("the address is the account's already").with_field("email"),
            );
        }
        let _request = self
            .inner
            .reset_limiter
            .admit_from_address(address, Instant::now())
            .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?;
        self.confirm_password(&user, password, WRONG, address)
            .await?;
        let changed_meanwhile =
            || AppError::conflict("the account was changed in the meantime; try again");
        let now = now_millis();
        if !self.config().mail_enabled() {
            let changed = self
                .store()
                .change_email_now(&user.id, &user.password_hash, email, now)
                .await
                .map_err(account_error)?
                .ok_or_else(changed_meanwhile)?;
            // Without mail the address before is told nothing, and no link gives it back.
            self.broadcast_person_change_in_background(changed.user_id);
            return Ok(Box::pin(async {}));
        }
        if !self
            .store()
            .ask_email_change(&user.id, &user.password_hash, email, now)
            .await?
        {
            return Err(changed_meanwhile());
        }
        let waiting = Waiting {
            user_id: user.id,
            username: user.username,
            email: email.to_owned(),
        };
        let (app, requested) = (self.clone(), Language::written_in(language));
        Ok(Box::pin(async move {
            app.mail_email_change(&waiting, requested).await;
        }))
    }

    /// Prepares a new mail of the link for the change the account of `session` waits
    /// for; the link of the mail before confirms nothing once this one is written. The
    /// returned work runs once the answer is handed off, as that of
    /// [`App::ask_email_change`].
    ///
    /// # Errors
    ///
    /// Returns 404 on a server that sends no mail and for an account that waits for
    /// no address, and 429 for too many requests from `address`.
    pub async fn prepare_email_change_mail(
        &self,
        session: &Session,
        language: &str,
        address: Option<IpAddr>,
    ) -> Result<impl Future<Output = ()> + Send + 'static + use<>, AppError> {
        if !self.config().mail_enabled() {
            return Err(AppError::not_found("route"));
        }
        let _request = self
            .inner
            .reset_limiter
            .admit_from_address(address, Instant::now())
            .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?;
        let user = self.store().get_user_by_id(&session.user_id).await?;
        let email = self
            .store()
            .email_change_of(&user.id, now_millis())
            .await?
            .ok_or_else(|| AppError::not_found("change of address"))?;
        let waiting = Waiting {
            user_id: user.id,
            username: user.username,
            email,
        };
        let (app, requested) = (self.clone(), Language::written_in(language));
        Ok(async move {
            app.mail_email_change(&waiting, requested).await;
        })
    }

    /// Ends what the account of `session` waits for: the link that was mailed confirms
    /// nothing any more. Nothing to end is no error.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn cancel_email_change(&self, session: &Session) -> Result<(), AppError> {
        self.store().cancel_email_change(&session.user_id).await?;
        Ok(())
    }

    /// Makes the address a link was mailed to the address of its account, for whoever
    /// holds the link's `token`: nobody is signed in by it. The account's reset links
    /// end, and the address it had is told that it is the account's no more, with the
    /// link that gives it back ([`App::revert_email_change`]).
    ///
    /// # Errors
    ///
    /// Returns 429 for too many requests from `address`, and 400 naming `token` for a
    /// link that changes nothing: unknown, used, replaced, cancelled or ended, of an
    /// account that is not in use, or to an address somebody took meanwhile (that link
    /// is left as it is, and changes the address once nobody has it). Which of them it
    /// was is not said.
    pub async fn confirm_email_change(
        &self,
        token: &str,
        address: Option<IpAddr>,
    ) -> Result<(), AppError> {
        let _attempt = self.admit_registration(address)?;
        let invalid = || {
            AppError::bad_request("the confirmation link is invalid or expired").with_field("token")
        };
        // The notice that carries the link back is on its way before the change is made,
        // or the change is not made: the link has no other way to its address.
        let notice = if self.config().mail_enabled() {
            Some(self.inner.mail.queue_email_changed().map_err(|_| {
                tracing::warn!("a change of address waits: the queue of its notice is full");
                AppError::service_unavailable("the server cannot change an address now; try again")
            })?)
        } else {
            None
        };
        let revert = new_secret();
        let changed = match self
            .store()
            .confirm_email_change(&token_digest(token), &token_digest(&revert), now_millis())
            .await
        {
            Ok(changed) => changed.ok_or_else(invalid)?,
            // Taken by a registration or an account made at the same moment.
            Err(fvoci_store::StoreError::Conflict(_)) => return Err(invalid()),
            Err(error) => return Err(error.into()),
        };
        self.broadcast_person_change_in_background(changed.user_id.clone());
        let Some(notice) = notice.filter(|_| !changed.old_email.is_empty()) else {
            return Ok(());
        };
        let link = SecretLink::new(
            self.settings().server_url(),
            SecretPage::RevertEmail,
            &revert,
        );
        let app = self.clone();
        self.spawn_background(async move {
            let language = app.mail_language(&changed.user_id, None).await;
            let content = Content::email_changed(&link, &changed.username, language);
            app.deliver_notice(notice, &changed.old_email, content)
                .await;
        });
        Ok(())
    }

    /// Gives the account of the link `token` the address the link was mailed to back,
    /// for whoever holds it: nobody is signed in by it. The account's sessions end and
    /// their connections close, and its password is taken nowhere until it is set
    /// anew ([`fvoci_store::Store::revert_email_change`] says what else goes). The
    /// address it had until then is told. Returns the address the account has again,
    /// or none where somebody else had it by now and the account's address was cleared.
    ///
    /// # Errors
    ///
    /// Returns 429 for too many requests from `address`, and 400 naming `token` for a
    /// link that changes nothing: unknown, used, ended, or of an account whose erasing
    /// began. Which of them it was is not said.
    pub async fn revert_email_change(
        &self,
        token: &str,
        address: Option<IpAddr>,
    ) -> Result<Option<String>, AppError> {
        let _attempt = self.admit_registration(address)?;
        let fvoci_store::EmailRevert { reverted, ended } = self
            .store()
            .revert_email_change(&token_digest(token), now_millis())
            .await?
            .ok_or_else(|| {
                AppError::bad_request("the link is invalid or expired").with_field("token")
            })?;
        self.ws().close(ended.into_scope());
        tracing::info!(
            user_id = %reverted.user_id,
            restored = reverted.restored.is_some(),
            unlinked_identities = reverted.unlinked_identities,
            "a link gave an account the address it had back"
        );
        self.broadcast_person_change_in_background(reverted.user_id.clone());
        let restored = reverted.restored.clone();
        if self.config().mail_enabled() && is_told(&reverted) {
            let app = self.clone();
            self.spawn_background(async move {
                let language = app.mail_language(&reverted.user_id, None).await;
                let content = Content::email_reverted(&reverted.username, language);
                app.mail_notice(&reverted.user_id, &reverted.before, content)
                    .await;
            });
        }
        Ok(restored)
    }

    /// The language of a mail to the account: the one of its settings if the mail is
    /// written in it, or else `requested`, or else English.
    async fn mail_language(&self, user_id: &str, requested: Option<Language>) -> Language {
        let own = match self.user_settings(user_id).await {
            Ok(settings) => Language::written_in(settings.language.as_str()),
            Err(error) => {
                tracing::warn!(%error, "the language of an account's mail could not be read");
                None
            }
        };
        Language::first_of([own, requested])
    }

    /// Mails the link that confirms the change the account waits for to its new
    /// address, unless somebody has that address: then nothing is mailed, and the
    /// account goes on waiting, so that what it is answered tells nothing. Every
    /// asking is one of the account's hour, and of the address's, before anything is
    /// known of the address, and one that mails nothing takes the turn of the pace a
    /// message would, so that the mail the account or anybody gets afterwards, and
    /// when, tells nothing either. What became of the message is logged, never told.
    async fn mail_email_change(&self, waiting: &Waiting, requested: Option<Language>) {
        if !self.inner.mail.count_email_change(&waiting.user_id) {
            tracing::info!("address change mail dropped: the account's hourly limit");
            return;
        }
        // Counted at the address, and in the queue, before anything is known of it.
        let queued = match self.inner.mail.queue_email_change(&waiting.email) {
            Ok(queued) => queued,
            Err(ResetMailDrop::AccountLimit) => {
                tracing::info!("address change mail dropped: the address's hourly limit");
                return;
            }
            Err(ResetMailDrop::QueueFull) => {
                tracing::warn!("address change mail dropped: its queue is full");
                return;
            }
        };
        match self
            .store()
            .email_is_held(&waiting.email, &waiting.user_id, now_millis())
            .await
        {
            Ok(false) => {}
            Ok(true) => {
                tracing::info!("no link mailed for a change of address: the address is held");
                // The turn of the pace a link would have taken is taken: how long the
                // next message to anybody waits tells nothing either.
                queued.pass(&self.inner.mail).await;
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "the link of a change of address was not mailed");
                return;
            }
        }
        let language = self.mail_language(&waiting.user_id, requested).await;
        // The digest of the link the message carries, once it is written: a retry is
        // sent only while that link still confirms the change.
        let digest = Mutex::new(String::new());
        let delivered = queued
            .deliver(
                &self.inner.mail,
                || async {
                    let token = new_secret();
                    let made = token_digest(&token);
                    // Not once the account waits for another address, or for none.
                    if !self
                        .store()
                        .renew_email_change_link(
                            &waiting.user_id,
                            &waiting.email,
                            &made,
                            now_millis(),
                        )
                        .await?
                    {
                        return Err(AppError::not_found("change of address"));
                    }
                    *digest.lock().expect(DIGEST_LOCK) = made;
                    let link = SecretLink::new(
                        self.settings().server_url(),
                        SecretPage::ConfirmEmail,
                        &token,
                    );
                    Ok::<_, AppError>(Letter {
                        to: waiting.email.clone(),
                        content: Content::email_change(&link, &waiting.username, language),
                    })
                },
                || async {
                    let digest = digest.lock().expect(DIGEST_LOCK).clone();
                    Ok(self
                        .store()
                        .email_change_link_stands(&waiting.user_id, &digest, now_millis())
                        .await?)
                },
            )
            .await;
        match delivered {
            Ok(_) => {}
            // Cancelled, replaced or ended while the message waited for its turn.
            Err(error) if error.is_not_found() => {}
            Err(error) => {
                tracing::warn!(%error, "the link of a change of address was not mailed");
            }
        }
    }

    /// Mails `content`, a notice about the account `user_id`, to `to`, within the
    /// account's hour of notices.
    async fn mail_notice(&self, user_id: &str, to: &str, content: Content) {
        let queued = match self.inner.mail.queue_email_notice(user_id) {
            Ok(queued) => queued,
            Err(ResetMailDrop::AccountLimit) => {
                tracing::warn!("notice of an address dropped: the account's hourly limit");
                return;
            }
            Err(ResetMailDrop::QueueFull) => {
                tracing::warn!("notice of an address dropped: its queue is full");
                return;
            }
        };
        self.deliver_notice(queued, to, content).await;
    }

    /// Mails `content`, a notice about an account, to `to` in the place `queued`.
    async fn deliver_notice(&self, queued: Queued, to: &str, content: Content) {
        let delivered = queued
            .deliver(
                &self.inner.mail,
                || async {
                    Ok::<_, AppError>(Letter {
                        to: to.to_owned(),
                        content,
                    })
                },
                // What it tells of has happened, and stays so.
                || async { Ok(true) },
            )
            .await;
        if let Err(error) = delivered {
            tracing::warn!(%error, "the notice of an address was not mailed");
        }
    }
}

/// Whether the address an account had before a revert link was used is told so:
/// where there was one, and it is no longer the account's address (restored to
/// another, or cleared).
fn is_told(reverted: &EmailReverted) -> bool {
    !reverted.before.is_empty()
        && reverted
            .restored
            .as_deref()
            .is_none_or(|now| !now.eq_ignore_ascii_case(&reverted.before))
}
