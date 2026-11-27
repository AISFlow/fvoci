//! Sending an existing invitation under the same permissions as making its link.

use fvoci_model::{Invitation, InvitationMail, WorkspaceRole};

use super::super::language::Language;
use super::super::mail::{Content, Delivered, InvitationRefusal, Letter, Organizations, Queued};
use super::{App, Inviting};
use crate::auth::{now_millis, secret::token_digest};
use crate::error::{AppError, Reason};
use fvoci_store::MailedTo;

impl App {
    /// Queues individual messages carrying this workspace's existing invitation.
    ///
    /// # Errors
    /// Returns 404 without mail or a usable link in this workspace, 403 when the
    /// sender cannot invite, 400 if any recipient is not a single email address or
    /// there are more than `invitation_mail_recipients`, 429 when the workspace's or the
    /// organization's hourly limit would be passed, and 503 when the invitation queue or,
    /// among several organizations, the organization's share of it cannot hold them
    /// all.
    /// Any refusal queues none of the messages.
    pub async fn email_invitation(
        &self,
        workspace_id: &str,
        sender: &str,
        mut request: InvitationMail,
    ) -> Result<(), AppError> {
        if !self.config().mail_enabled() {
            return Err(AppError::not_found("route"));
        }
        let limit = self.config().smtp.invitation_mail_recipients;
        if !u32::try_from(request.recipients.len()).is_ok_and(|named| named <= limit) {
            return Err(AppError::bad_request(format!(
                "at most {limit} recipients may be named at once"
            ))
            .with_field("recipients")
            .with_reason(Reason::TooManyRecipients)
            .with_limit(u64::from(limit)));
        }
        let mut recipients = std::collections::BTreeSet::new();
        for recipient in &request.recipients {
            let normalized = recipient.trim().to_owned();
            if recipient.contains(['\r', '\n'])
                || !fvoci_model::is_email_valid(&normalized)
                || normalized.parse::<lettre::Address>().is_err()
            {
                return Err(AppError::bad_request(
                    "every recipient must be a single email address",
                )
                .with_field("recipients"));
            }
            recipients.insert(with_lowercase_domain(&normalized));
        }
        if recipients.is_empty() {
            return Err(AppError::bad_request("at least one recipient is required")
                .with_field("recipients"));
        }
        self.invitation_for_mail(workspace_id, sender, &request.token)
            .await?;
        // Distinct recipients are no more than those named, which fit in the limit.
        let count = u32::try_from(recipients.len()).unwrap_or(limit);
        // The handle is the organization's the workspace is in: what it holds of the
        // mail is counted under it, at its own limit.
        let Some(org) = self.store().tenant().org_id() else {
            return Err(AppError::internal(
                "invitation mail outside an organization",
            ));
        };
        let organizations = self.organizations().await?;
        let queued = self
            .inner
            .mail
            .queue_invitations(
                workspace_id,
                &org,
                self.limits().invitation_mail_per_hour,
                organizations,
                count,
            )
            .map_err(|refused| match refused {
                InvitationRefusal::WorkspaceHourlyLimit(wait) => AppError::quota_reached(
                    "the workspace sent its hourly invitation mail; try again later",
                    Reason::InvitationMailLimit,
                    wait,
                ),
                InvitationRefusal::OrgHourlyLimit(wait) => AppError::quota_reached(
                    "the organization sent its hourly invitation mail; try again later",
                    Reason::InvitationMailLimit,
                    wait,
                ),
                InvitationRefusal::OrgShareFull => AppError::service_unavailable(
                    "the organization has too much mail waiting; try again later",
                )
                .with_reason(Reason::MailQueueFull),
                InvitationRefusal::QueueFull => {
                    AppError::service_unavailable("too much mail is waiting; try again later")
                        .with_reason(Reason::MailQueueFull)
                }
            })?;
        request.recipients = recipients.into_iter().collect();
        let admitted = self
            .inner
            .mail
            .admit_invitation_addresses(&request.recipients);
        let app = self.clone();
        let workspace_id = workspace_id.to_owned();
        let sender = sender.to_owned();
        self.spawn_background(async move {
            // The cause is the store's or a refusal: what became of each message is told
            // where it is sent.
            if let Err(error) = app
                .deliver_invitation(&workspace_id, &sender, request, queued, admitted)
                .await
            {
                tracing::warn!(%error, "invitation mail delivery failed");
            }
        });
        Ok(())
    }

    /// Whether the server holds one organization or several, as the invitation queue
    /// shares itself: one on SQLite, which holds one; on PostgreSQL, one when one only is
    /// in use, whatever `org_creation` says now (it may have been `open` before).
    async fn organizations(&self) -> Result<Organizations, AppError> {
        if self.settings().database_kind() == fvoci_store::DbType::Sqlite {
            return Ok(Organizations::One);
        }
        // What a login page shows without a name: the only organization in use, if
        // there is one only.
        Ok(if self.store().login_organization(None).await?.is_some() {
            Organizations::One
        } else {
            Organizations::Several
        })
    }

    async fn invitation_for_mail(
        &self,
        workspace_id: &str,
        sender: &str,
        token: &str,
    ) -> Result<Invitation, AppError> {
        // A member link is mailed as it is, so it must let somebody in still; a guest
        // link is only what each recipient's own link is made like, and goes out after
        // it was used too (while it is neither revoked nor ended).
        let invitation = self
            .store()
            .unended_invitation(&token_digest(token), now_millis())
            .await?
            .filter(|invitation| invitation.workspace_id == workspace_id)
            .filter(|invitation| {
                invitation.role == WorkspaceRole::Guest
                    || invitation
                        .max_uses
                        .is_none_or(|most| invitation.uses < most)
            })
            .ok_or_else(|| AppError::not_found("invitation"))?;
        if self
            .inviter_who_may_still_invite(&invitation)
            .await?
            .is_none()
        {
            return Err(AppError::not_found("invitation"));
        }
        let mut by_sender = invitation.clone();
        sender.clone_into(&mut by_sender.created_by);
        if self
            .inviter_who_may_still_invite(&by_sender)
            .await?
            .is_none()
        {
            return Err(AppError::forbidden(
                "only who may make this invitation may email it",
            ));
        }
        Ok(invitation)
    }

    async fn deliver_invitation(
        &self,
        workspace_id: &str,
        sender: &str,
        request: InvitationMail,
        queued: Vec<Queued>,
        admitted: Vec<bool>,
    ) -> Result<(), AppError> {
        let sender_language =
            Language::written_in(self.user_settings(sender).await?.language.as_str());
        let requested = Language::written_in(&request.language);
        let server = self.settings().server_url();
        for ((recipient, place), allowed) in
            request.recipients.into_iter().zip(queued).zip(admitted)
        {
            if !allowed {
                place.pass(&self.inner.mail).await;
                continue;
            }
            // The guest link made for this recipient, if one was: its identifier and
            // its token.
            let made: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::default();
            let delivered = place
                .deliver(
                    &self.inner.mail,
                    || async {
                        // Permissions and expiry may have changed while this waited its turn.
                        let invitation = self
                            .invitation_for_mail(workspace_id, sender, &request.token)
                            .await?;
                        let recipient_language =
                            match self.store().get_user_by_email(&recipient).await {
                                Ok(user) => Language::written_in(
                                    self.user_settings(&user.id).await?.language.as_str(),
                                ),
                                Err(error) if error.is_not_found() => None,
                                Err(error) => return Err(AppError::from(error)),
                            };
                        let language =
                            Language::first_of([recipient_language, sender_language, requested]);
                        let token = match invitation.role {
                            // A guest link lets one person in once: each recipient has their
                            // own, made by the sender as their message leaves.
                            WorkspaceRole::Guest => {
                                let link = self
                                    .guest_link_like(&invitation, sender, &recipient)
                                    .await?;
                                let token = link.token.ok_or_else(|| {
                                    AppError::internal("a new link without its token")
                                })?;
                                *made
                                    .lock()
                                    .expect("the link made is never held across a panic") =
                                    Some((link.id, token.clone()));
                                token
                            }
                            // Members share the one link.
                            _ => request.token.clone(),
                        };
                        let link = server.at(&format!("/invite/{token}"));
                        Ok(Letter {
                            content: Content::invitation(&link, language, invitation.role),
                            to: recipient,
                        })
                    },
                    // A message held a while (a closed way) goes only while what it was written
                    // of stands: its link, and who may send it.
                    || self.invitation_mail_stands(workspace_id, sender, &request.token, &made),
                )
                .await?;
            // Nobody holds the link of a message refused for good or given up: it goes
            // with it, and the sender may mail the link again. A failure here is logged;
            // the other recipients are still sent theirs.
            let made = made
                .into_inner()
                .expect("the link made is never held across a panic");
            if matches!(
                delivered,
                Delivered::Refused | Delivered::GivenUp | Delivered::Withdrawn
            ) && let Some((id, _)) = made
            {
                self.revoke_undelivered_guest_link(workspace_id, sender, &id)
                    .await;
            }
        }
        Ok(())
    }

    async fn revoke_undelivered_guest_link(&self, workspace_id: &str, sender: &str, id: &str) {
        match self
            .store()
            .revoke_invitation(workspace_id, id, Some(sender), now_millis())
            .await
        {
            Ok(true) => tracing::info!(
                invitation_id = %id,
                workspace_id,
                "a link nobody received was revoked"
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                %error,
                invitation_id = %id,
                "a link nobody received could not be revoked"
            ),
        }
    }

    /// Whether an invitation message written a while ago still stands: the link it was
    /// mailed of is neither revoked nor ended and its sender may still send it, and the
    /// guest link made for its recipient, if one was, is live still.
    ///
    /// # Errors
    /// Returns what failed other than a refusal.
    async fn invitation_mail_stands(
        &self,
        workspace_id: &str,
        sender: &str,
        token: &str,
        made: &std::sync::Mutex<Option<(String, String)>>,
    ) -> Result<bool, AppError> {
        match self.invitation_for_mail(workspace_id, sender, token).await {
            Ok(_) => {}
            Err(error) if error.is_refusal() => return Ok(false),
            Err(error) => return Err(error),
        }
        let child = made
            .lock()
            .expect("the link made is never held across a panic")
            .as_ref()
            .map(|(_, token)| token_digest(token));
        let Some(child) = child else {
            return Ok(true);
        };
        Ok(self
            .store()
            .live_invitation(&child, now_millis(), None)
            .await?
            .is_some())
    }

    /// A new one-use guest link like `invitation` (its board, its role on it and its
    /// end), made by `sender` for `recipient` of its mail, with its token, and recorded
    /// as `invitation`'s child mailed to `recipient`. The caller has asked whether
    /// `sender` may make such a link.
    async fn guest_link_like(
        &self,
        invitation: &Invitation,
        sender: &str,
        recipient: &str,
    ) -> Result<Invitation, AppError> {
        let (Some(container_id), Some(role)) = (
            invitation.container_id.as_deref(),
            invitation.container_role,
        ) else {
            return Err(AppError::internal(
                "a guest link without its board and role",
            ));
        };
        self.store_link(
            &invitation.workspace_id,
            sender,
            Inviting::Guest { container_id, role },
            Some(invitation.expires_at),
            Some(MailedTo {
                parent_id: &invitation.id,
                recipient,
            }),
        )
        .await
    }
}

/// The address with its domain in lower case, which names the same mailbox; the local
/// part keeps its case, which a mail server may tell apart.
fn with_lowercase_domain(address: &str) -> String {
    match address.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => address.to_owned(),
    }
}
