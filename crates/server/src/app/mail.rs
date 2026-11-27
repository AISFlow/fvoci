//! Outgoing mail in tracked background work: bounded queues, shares of the sending, one
//! pace for all, and bounded attempts. How a message leaves is the [`Outbound`]'s.

mod content;
mod mime;
mod outbound;
mod pace;
mod provider;
mod quota;
mod secret_link;
mod share;
mod smtp;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::auth::limiter::RetryAfter;
use crate::config::mail::{MailSettings, Way, is_loopback_url};
use crate::config::smtp::SmtpSettings;
use crate::metrics::{MailFailure, MailKind, Metrics, ResetMailDrop};

pub(super) use content::Content;
use outbound::{Mailbox, Outbound, Outcome, Outgoing};
pub(super) use secret_link::{SecretLink, SecretPage};
pub(super) use share::Organizations;

/// At most four messages are being sent at once; HTTP requests never wait for them.
const CONNECTIONS: usize = 4;
/// Invitations use at most this many of them, so one is always left for recovery.
const INVITATION_CONNECTIONS: usize = CONNECTIONS - 1;
/// One attempt, including connecting and every command or request, gets ten seconds.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
/// The waits before the second and the third attempt, when the way did not say.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(4)];
/// What a mail semaphore's acquire expects: none is ever closed.
const CLOSED: &str = "the mail semaphores are never closed";
/// The longest a message waits for another attempt, as long as invitations are held
/// back at most: a way that asks for longer has its message given up then, rather than
/// asked again before it said. SES asks to wait up to ten minutes when it throttles.
const LONGEST_RETRY_WAIT: Duration = Duration::from_mins(15);
/// The longest a recovery message waits for a closed way to open, from the first of its
/// attempts the way refused so: a reset link is good for an hour, and the queue's
/// places are for mail that may still go. Invitations wait as long as it stays closed.
const LONGEST_PATH_WAIT: Duration = Duration::from_mins(15);

/// How many messages are queued and not yet delivered or given up, as
/// [`App::unsent_mail`](super::App::unsent_mail) counts them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnsentMail {
    /// Password resets and confirmations of a registration, which share a queue.
    pub recovery: usize,
    /// Invitations, which have a queue of their own.
    pub invitations: usize,
}

impl UnsentMail {
    /// Whether no message is waiting.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// Why invitation messages were not queued; nothing of the request was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InvitationRefusal {
    /// The workspace queued its hourly limit; the oldest of those leave after this.
    WorkspaceHourlyLimit(Duration),
    /// The organization queued its hourly limit; the oldest of those leave after this.
    OrgHourlyLimit(Duration),
    /// The organization holds its share of the invitation queue.
    OrgShareFull,
    /// The invitation queue holds as many messages as it may.
    QueueFull,
}

/// The optional way of sending, with the queues, shares and pace in front of it;
/// constructing it performs no network I/O.
#[derive(Clone)]
pub(super) struct Mailer {
    /// The way of sending, or `None` for no mail.
    sender: Option<Sender>,
    connections: Arc<Semaphore>,
    invitation_connections: Arc<Semaphore>,
    reset_queue: Arc<Semaphore>,
    /// The places of `reset_queue` (`mail_reset_queue`).
    reset_places: usize,
    invitation_queue: Arc<Semaphore>,
    /// The places of `invitation_queue` (`mail_invitation_queue`).
    invitation_places: usize,
    /// Invitation messages per workspace and hour, up to `invitation_mail_per_hour`.
    workspace_quota: Arc<quota::HourlyQuota>,
    workspace_per_hour: u32,
    /// Invitation messages per organization and hour, up to the organization's limit.
    org_quota: Arc<quota::HourlyQuota>,
    org_shares: Arc<share::OrgShares>,
    /// Invitation messages per recipient email address and hour across all organizations,
    /// up to `invitation_mail_per_address_hour`.
    invitation_address_quota: Arc<quota::HourlyQuota>,
    invitation_address_per_hour: u32,
    /// Recovery messages per email address and hour, up to `mail_reset_per_account_hour`.
    reset_quota: Arc<quota::HourlyQuota>,
    /// Messages that confirm a registration's address, per address and hour, up to the
    /// same number; they go in the recovery queue.
    verification_quota: Arc<quota::HourlyQuota>,
    /// Askings for a link that confirms an account's change of its address, per account
    /// and hour, up to the same number: every asking counts, whether or not a link is
    /// mailed for it; a link that is counts at its address too (`verification_quota`).
    change_quota: Arc<quota::HourlyQuota>,
    /// Notices of a changed address, per account and hour, up to the same number:
    /// counted apart, so that the links an account asked for never use up its notice.
    notice_quota: Arc<quota::HourlyQuota>,
    reset_per_hour: u32,
    pace: Arc<pace::Pace>,
    metrics: Metrics,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer")
            .field("enabled", &self.sender.is_some())
            .finish_non_exhaustive()
    }
}

impl Mailer {
    /// `server_url` is where the links of the mail lead; the way of sending is the one
    /// `mail` names (`mail_transport`, or SMTP when `smtp_host` is set).
    ///
    /// # Errors
    /// Returns an error for settings that are invalid or cannot make the way.
    pub(super) fn new(
        settings: &SmtpSettings,
        mail: &MailSettings,
        server_url: &str,
        metrics: Metrics,
    ) -> anyhow::Result<Self> {
        settings.check()?;
        let way = mail.way(settings)?;
        if way.is_some() && is_loopback_root(server_url) {
            tracing::warn!(
                server_url,
                "mail is sent, but server_url is a loopback address: its links lead nowhere \
                 for their recipients"
            );
        }
        let outbound: Option<Arc<dyn Outbound>> = match way {
            None => None,
            Some(Way::Smtp) => Some(Arc::new(smtp::Smtp::new(settings, ATTEMPT_TIMEOUT)?)),
            Some(Way::Cloudflare(keys)) => Some(Arc::new(provider::cloudflare::Cloudflare::new(
                keys,
                provider::client(ATTEMPT_TIMEOUT)?,
            ))),
            Some(Way::Ses(keys)) => Some(Arc::new(provider::ses::Ses::new(
                keys,
                provider::client(ATTEMPT_TIMEOUT)?,
            )?)),
            Some(Way::Oci(keys)) => Some(Arc::new(provider::oci::Oci::new(
                keys,
                provider::client(ATTEMPT_TIMEOUT)?,
            )?)),
        };
        Self::with_outbound(settings, outbound, metrics)
    }

    /// The mailer of `settings` that sends through `outbound`, or sends nothing.
    fn with_outbound(
        settings: &SmtpSettings,
        outbound: Option<Arc<dyn Outbound>>,
        metrics: Metrics,
    ) -> anyhow::Result<Self> {
        let reset_places = usize::try_from(settings.mail_reset_queue)?;
        let invitation_places = usize::try_from(settings.mail_invitation_queue)?;
        let sender = match outbound {
            Some(outbound) => Some(Sender {
                outbound,
                from: settings
                    .mail_from
                    .parse()
                    .context("mail_from is no address mail is sent from")?,
            }),
            None => None,
        };
        Ok(Self {
            sender,
            connections: Arc::new(Semaphore::new(CONNECTIONS)),
            invitation_connections: Arc::new(Semaphore::new(INVITATION_CONNECTIONS)),
            reset_queue: Arc::new(Semaphore::new(reset_places)),
            reset_places,
            invitation_queue: Arc::new(Semaphore::new(invitation_places)),
            invitation_places,
            workspace_quota: Arc::default(),
            workspace_per_hour: settings.invitation_mail_per_hour,
            org_quota: Arc::default(),
            org_shares: Arc::new(share::OrgShares::new(
                settings.mail_invitation_queue,
                settings.invitation_mail_recipients,
            )),
            invitation_address_quota: Arc::default(),
            invitation_address_per_hour: settings.invitation_mail_per_address_hour,
            reset_quota: Arc::default(),
            verification_quota: Arc::default(),
            change_quota: Arc::default(),
            notice_quota: Arc::default(),
            reset_per_hour: settings.mail_reset_per_account_hour,
            pace: Arc::new(pace::Pace::new(settings.mail_per_second)),
            metrics,
        })
    }

    /// The messages that hold a place in a queue now: waiting for their turn or for
    /// another attempt, or being sent. They are kept in memory only, so a server that
    /// stops with some loses them.
    pub(super) fn unsent(&self) -> UnsentMail {
        UnsentMail {
            recovery: self
                .reset_places
                .saturating_sub(self.reset_queue.available_permits()),
            invitations: self
                .invitation_places
                .saturating_sub(self.invitation_queue.available_permits()),
        }
    }

    /// Takes the place of one recovery message to `email` (in any letter case) in its
    /// queue, before the account is looked up. A message that finds no place counts
    /// nothing against the address's hour.
    ///
    /// # Errors
    /// Returns why the message is dropped: the hourly limit, asked first, or a full queue.
    pub(super) fn queue_reset(&self, email: &str) -> Result<Queued, ResetMailDrop> {
        let key = email.to_lowercase();
        self.reset_quota
            .take(&key, 1, self.reset_per_hour, Instant::now(), || {
                Arc::clone(&self.reset_queue).try_acquire_owned()
            })
            .map(|place| Queued {
                kind: MailKind::PasswordReset,
                place,
                org: None,
            })
            .map_err(|refusal| dropped(&refusal))
    }

    /// Takes the place of one message that confirms the address `email` of a
    /// registration, in the recovery queue: it goes before invitations as a reset link
    /// does. The address's hour is counted as an account's recovery messages are.
    ///
    /// # Errors
    /// Returns why the message is dropped, as [`Mailer::queue_reset`] does.
    pub(super) fn queue_verification(&self, email: &str) -> Result<Queued, ResetMailDrop> {
        let key = email.to_lowercase();
        self.verification_quota
            .take(&key, 1, self.reset_per_hour, Instant::now(), || {
                Arc::clone(&self.reset_queue).try_acquire_owned()
            })
            .map(|place| Queued {
                kind: MailKind::EmailVerification,
                place,
                org: None,
            })
            .map_err(|refusal| dropped(&refusal))
    }

    /// Counts one link the account `user_id` asks for in the account's hour, whatever
    /// becomes of it: mailed, left out because the address is somebody's, or dropped
    /// for the address's own hour or a full queue. So what the account is mailed
    /// afterwards tells nothing of the address it asked for. False once the account
    /// has had the number an hour: nothing is mailed then.
    pub(super) fn count_email_change(&self, user_id: &str) -> bool {
        self.change_quota
            .take(user_id, 1, self.reset_per_hour, Instant::now(), || {
                Ok::<(), std::convert::Infallible>(())
            })
            .is_ok()
    }

    /// Takes the place of one message that carries the link confirming a change to
    /// the address `email`, in the recovery queue. The address's hour is counted, with
    /// the messages that confirm a registration's: an address is sent no more than the
    /// number an hour, whoever asks. The account's hour is counted before, apart
    /// ([`Self::count_email_change`]).
    ///
    /// # Errors
    /// Returns why the message is dropped, as [`Mailer::queue_reset`] does.
    pub(super) fn queue_email_change(&self, email: &str) -> Result<Queued, ResetMailDrop> {
        self.verification_quota
            .take(
                &email.to_lowercase(),
                1,
                self.reset_per_hour,
                Instant::now(),
                || Arc::clone(&self.reset_queue).try_acquire_owned(),
            )
            .map(|place| Queued {
                kind: MailKind::EmailChange,
                place,
                org: None,
            })
            .map_err(|refusal| dropped(&refusal))
    }

    /// Takes the place of the notice that carries the link giving an account the
    /// address it had back, in the recovery queue, before the change it tells of is
    /// made. No hour is counted: the changes are, by the links mailed for them
    /// ([`Self::count_email_change`]).
    ///
    /// # Errors
    /// Returns [`ResetMailDrop::QueueFull`] while the queue is.
    pub(super) fn queue_email_changed(&self) -> Result<Queued, ResetMailDrop> {
        Arc::clone(&self.reset_queue)
            .try_acquire_owned()
            .map(|place| Queued {
                kind: MailKind::EmailChange,
                place,
                org: None,
            })
            .map_err(|_| ResetMailDrop::QueueFull)
    }

    /// Takes the place of one notice to the account `user_id` that its address was
    /// given back, in the recovery queue; the account's hour of notices is counted.
    ///
    /// # Errors
    /// Returns why the message is dropped, as [`Mailer::queue_reset`] does.
    pub(super) fn queue_email_notice(&self, user_id: &str) -> Result<Queued, ResetMailDrop> {
        self.notice_quota
            .take(user_id, 1, self.reset_per_hour, Instant::now(), || {
                Arc::clone(&self.reset_queue).try_acquire_owned()
            })
            .map(|place| Queued {
                kind: MailKind::EmailChange,
                place,
                org: None,
            })
            .map_err(|refusal| dropped(&refusal))
    }

    /// Takes the places of all `messages` of one invitation request of the workspace
    /// `workspace_id` in the organization `org_id`, or none of them. In this order, the
    /// workspace's hourly limit, the organization's (`org_per_hour`), the
    /// organization's share of the queue among the server's `organizations` and the
    /// queue must each take them all: a workspace or an organization past its hour is
    /// told when to come back even while the queue is full, and an organization of
    /// several never holds so much that one more finds no room.
    pub(super) fn queue_invitations(
        &self,
        workspace_id: &str,
        org_id: &str,
        org_per_hour: u32,
        organizations: Organizations,
        messages: u32,
    ) -> Result<Vec<Queued>, InvitationRefusal> {
        let now = Instant::now();
        let (orgs, mut places) = self
            .workspace_quota
            .take(workspace_id, messages, self.workspace_per_hour, now, || {
                self.org_quota
                    .take(org_id, messages, org_per_hour, now, || {
                        let orgs = self
                            .org_shares
                            .take(org_id, messages, organizations)
                            .ok_or(InvitationRefusal::OrgShareFull)?;
                        let places = Arc::clone(&self.invitation_queue)
                            .try_acquire_many_owned(messages)
                            .map_err(|_| InvitationRefusal::QueueFull)?;
                        Ok((orgs, places))
                    })
                    .map_err(|refusal| match refusal {
                        quota::Refusal::Limit(RetryAfter(wait)) => {
                            InvitationRefusal::OrgHourlyLimit(wait)
                        }
                        quota::Refusal::Unplaced(refusal) => refusal,
                    })
            })
            .map_err(|refusal| match refusal {
                quota::Refusal::Limit(RetryAfter(wait)) => {
                    InvitationRefusal::WorkspaceHourlyLimit(wait)
                }
                quota::Refusal::Unplaced(refusal) => refusal,
            })?;
        let mut queued = Vec::new();
        for org in orgs {
            let Some(place) = places.split(1) else { break };
            queued.push(Queued {
                kind: MailKind::Invitation,
                place,
                org: Some(org),
            });
        }
        Ok(queued)
    }

    /// Checks each recipient email address (case-insensitively) against the server-wide
    /// `invitation_mail_per_address_hour` window, returning `true` for addresses that fit
    /// and `false` for addresses that have already received their hourly quota.
    pub(super) fn admit_invitation_addresses(&self, recipients: &[String]) -> Vec<bool> {
        let lower_keys: Vec<String> = recipients.iter().map(|addr| addr.to_lowercase()).collect();
        self.invitation_address_quota.take_each(
            &lower_keys,
            self.invitation_address_per_hour,
            Instant::now(),
        )
    }
}

/// Why a message that found its hour used up, or no place in the recovery queue, is
/// dropped.
fn dropped<E>(refusal: &quota::Refusal<E>) -> ResetMailDrop {
    match refusal {
        quota::Refusal::Limit(_) => ResetMailDrop::AccountLimit,
        quota::Refusal::Unplaced(_) => ResetMailDrop::QueueFull,
    }
}

/// Whether `server_url` names no address its recipients reach, as the default does:
/// `localhost`, a loopback address (also one mapped into IPv6), or the unspecified one.
/// A root that is no URL is left to the configuration's own checks.
fn is_loopback_root(server_url: &str) -> bool {
    reqwest::Url::parse(server_url).is_ok_and(|url| is_loopback_url(&url))
}

/// One message's place in its queue, held until the message was delivered or given up.
#[derive(Debug)]
pub(super) struct Queued {
    kind: MailKind,
    place: OwnedSemaphorePermit,
    /// For an invitation, its part of its organization's share.
    org: Option<share::OrgPlace>,
}

impl Queued {
    /// Takes the turn of the pace the message would have taken to leave, and sends
    /// nothing: for a message that is left out where the mail that others are sent
    /// afterwards must not tell it from one that is sent. While the way is closed no
    /// message leaves to be timed, and the turn is left to one that may.
    pub(super) async fn pass(self, mailer: &Mailer) {
        mailer.pace.pass_turn(self.kind).await;
    }
}

/// A message the caller made, to one recipient; the sender is the mailer's.
pub(super) struct Letter {
    pub(super) to: String,
    pub(super) content: Content,
}

impl Queued {
    /// Delivers the message `write` makes, keeping the place until it was delivered or
    /// given up. Each attempt waits for its turn of the pace and then for a connection;
    /// an invitation holds its organization's sending and one of the invitations' while
    /// it waits for the turn, and a recovery message nothing; between attempts nothing
    /// is held. The first attempt only then asks `write` for the message, so that what
    /// `write` creates (a reset token) is as young as can be.
    /// Attempts follow the way's [`Outcome`]: up to three while it asks for later, one
    /// when it refuses; while the way is closed, the message waits for the pace's next
    /// attempt at it, which counts against none of the three (a recovery message for
    /// up to [`LONGEST_PATH_WAIT`]). Each attempt after the first asks `still` whether the
    /// message stands, and stops if not. What became of the message is logged and counted here,
    /// without a reply text, an address or a token.
    ///
    /// # Errors
    /// Returns what `write` returned.
    pub(super) async fn deliver<E, Writing, Checking>(
        self,
        mailer: &Mailer,
        write: impl FnOnce() -> Writing,
        mut still: impl FnMut() -> Checking,
    ) -> Result<Delivered, E>
    where
        Writing: Future<Output = Result<Letter, E>>,
        Checking: Future<Output = Result<bool, E>>,
    {
        let Some(sender) = mailer.sender.as_ref() else {
            tracing::warn!("a message was queued while outgoing mail is disabled");
            return Ok(Delivered::Unwritten);
        };
        let outbound = &sender.outbound;
        let mut delivered = Delivered::Unwritten;
        let kind = self.kind;
        let mut write = Some(write);
        let mut message = None;
        let mut tried = Tried {
            waits: RETRY_DELAYS.into_iter(),
            closed_since: None,
        };
        for attempt in 1.. {
            // A recovery message's quarter of an hour starts when it first waits for a
            // closed way, whether or not an attempt of its own found it so.
            if kind.goes_first() && mailer.pace.is_closed() {
                tried
                    .closed_since
                    .get_or_insert_with(tokio::time::Instant::now);
            }
            // An invitation's sendings first, its organization's and the invitations':
            // they keep waiting only invitations, and an invitation that has its turn
            // has them, so it neither spends a turn waiting nor uses one after a pause.
            let held = mailer.invitation_sendings(self.org.as_ref()).await;
            if !mailer.turn(kind, tried.closed_since).await {
                delivered = Delivered::GivenUp;
                tracing::warn!(
                    kind = kind.as_str(),
                    way = outbound.name(),
                    attempt,
                    "mail given up: the way stayed closed"
                );
                mailer.metrics.mail_failed(kind, MailFailure::Temporary);
                break;
            }
            let sending = mailer.sending(held).await;
            let ready: Option<&Outgoing> = if let Some(ref message) = message {
                // What it was written of may have changed while it waited (a closed way
                // holds an invitation for as long as it stays closed).
                if !still().await? {
                    delivered = Delivered::Withdrawn;
                    tracing::info!(
                        kind = kind.as_str(),
                        attempt,
                        "mail withdrawn: it no longer stands"
                    );
                    break;
                }
                Some(message)
            } else {
                let write = write
                    .take()
                    .expect("the message is written once, at the first attempt");
                sender
                    .outgoing(write().await?)
                    .map(|written| &*message.insert(written))
            };
            let outcome = match ready {
                Some(message) => tokio::time::timeout(ATTEMPT_TIMEOUT, outbound.send(message))
                    .await
                    .unwrap_or(Outcome::Later {
                        after: None,
                        code: None,
                    }),
                // A recipient's address that is none: every way refuses it.
                None => Outcome::Refused { code: None },
            };
            drop(sending);
            let way = outbound.name();
            mailer.observe(way, outcome);
            match mailer.after_attempt(
                kind,
                Attempt {
                    way,
                    number: attempt,
                    outcome,
                },
                &mut tried,
            ) {
                Next::Done(done) => {
                    delivered = done;
                    break;
                }
                Next::Wait(wait) => tokio::time::sleep(wait).await,
            }
        }
        drop((self.place, self.org));
        Ok(delivered)
    }
}

/// What a message has had of its attempts so far.
struct Tried {
    /// The waits before the attempts that are left after the way asked for later.
    waits: std::array::IntoIter<Duration, 2>,
    /// Since when the way has been closed to this message's attempts, if it is.
    closed_since: Option<tokio::time::Instant>,
}

/// One attempt and what it became.
#[derive(Clone, Copy)]
struct Attempt {
    way: &'static str,
    /// The attempt's number, from 1.
    number: u32,
    outcome: Outcome,
}

/// What a message does after an attempt.
enum Next {
    /// No more attempts: this became of it.
    Done(Delivered),
    /// Another attempt, after this; a closed way's attempt waits for the pace instead.
    Wait(Duration),
}

impl Mailer {
    /// Waits for the turn of a message of `kind`. A recovery message that the way has
    /// been closed to since `closed_since` waits until [`LONGEST_PATH_WAIT`] from then
    /// at most: its link ages, and its place is for mail that may still go. An
    /// invitation waits for as long as the way stays closed. Returns whether the turn
    /// came.
    async fn turn(&self, kind: MailKind, closed_since: Option<tokio::time::Instant>) -> bool {
        let turn = self.pace.turn(kind);
        match closed_since {
            Some(since) if kind.goes_first() => {
                tokio::time::timeout_at(since + LONGEST_PATH_WAIT, turn)
                    .await
                    .is_ok()
            }
            _ => {
                turn.await;
                true
            }
        }
    }

    /// Decides what follows `attempt` of a message of `kind`, and logs and counts a
    /// message given up.
    fn after_attempt(&self, kind: MailKind, attempt: Attempt, tried: &mut Tried) -> Next {
        let Attempt {
            way,
            number: attempt,
            outcome,
        } = attempt;
        if !matches!(outcome, Outcome::PathDown { .. }) {
            tried.closed_since = None;
        }
        match outcome {
            Outcome::Sent => Next::Done(Delivered::Sent),
            Outcome::Refused { code } => {
                tracing::warn!(
                    kind = kind.as_str(),
                    way,
                    attempt,
                    code,
                    "mail refused for good"
                );
                self.metrics.mail_failed(kind, MailFailure::Permanent);
                Next::Done(Delivered::Refused)
            }
            Outcome::Later { after, code } => {
                tracing::warn!(kind = kind.as_str(), way, attempt, code, "mail deferred");
                // A way that asks for longer than another attempt may wait is not asked
                // again before it said.
                let wait = tried.waits.next().map(|backoff| after.unwrap_or(backoff));
                if let Some(wait) = wait.filter(|wait| *wait <= LONGEST_RETRY_WAIT) {
                    Next::Wait(wait)
                } else {
                    tracing::warn!(kind = kind.as_str(), way, attempt, "mail given up");
                    self.metrics.mail_failed(kind, MailFailure::Temporary);
                    Next::Done(Delivered::GivenUp)
                }
            }
            Outcome::PathDown { .. } => {
                tried
                    .closed_since
                    .get_or_insert_with(tokio::time::Instant::now);
                // The pace holds the next attempt until the way may be tried again, and
                // [`Mailer::turn`] ends a recovery message's wait.
                Next::Wait(Duration::ZERO)
            }
        }
    }
}

/// What became of a queued message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Delivered {
    /// The way took it.
    Sent,
    /// The way refused it for good.
    Refused,
    /// The way asked for later at every attempt, or for longer than it may wait.
    GivenUp,
    /// Its next attempt found that it no longer stands (its link revoked or ended,
    /// its sender no longer allowed): nothing more was sent.
    Withdrawn,
    /// Nothing was sent: mail is off.
    Unwritten,
}

/// What an invitation holds of the sending before its turn: its organization's sending
/// and one of the invitations'; nothing for a recovery message. Given back in the order
/// of the fields, after the connection.
struct Held {
    _invitations: Option<OwnedSemaphorePermit>,
    _org: Option<OwnedSemaphorePermit>,
}

/// The sendings one attempt holds while it is made; the connection is given back first,
/// before the shares that let an invitation take it, so invitations never hold all four
/// at once, nor one organization all three of theirs.
struct Sending {
    _connection: OwnedSemaphorePermit,
    _held: Held,
}

impl Mailer {
    /// Waits for what an invitation of the organization's share `org` holds before its
    /// turn; a recovery message (no `org`) waits for nothing.
    async fn invitation_sendings(&self, org: Option<&share::OrgPlace>) -> Held {
        let Some(org) = org else {
            return Held {
                _invitations: None,
                _org: None,
            };
        };
        let org = Arc::clone(&org.sending)
            .acquire_owned()
            .await
            .expect(CLOSED);
        let invitations = Arc::clone(&self.invitation_connections)
            .acquire_owned()
            .await
            .expect(CLOSED);
        Held {
            _invitations: Some(invitations),
            _org: Some(org),
        }
    }

    /// Waits for a connection, beside what the message `held` already.
    async fn sending(&self, held: Held) -> Sending {
        Sending {
            _connection: Arc::clone(&self.connections)
                .acquire_owned()
                .await
                .expect(CLOSED),
            _held: held,
        }
    }

    /// Tells the pace what an attempt through `way` became, and logs and counts the way
    /// closing or opening: one line each time, whatever many messages found it so.
    fn observe(&self, way: &'static str, outcome: Outcome) {
        match self.pace.observe(outcome) {
            Some(pace::PathChange::Closed) => {
                let code = match outcome {
                    Outcome::PathDown { code } => code,
                    _ => None,
                };
                tracing::error!(
                    way,
                    code,
                    "the way of sending is closed: it refuses the server's keys or \
                     permissions, or cannot be reached; invitations wait and one message \
                     is tried a minute"
                );
                self.metrics.mail_path_closed();
            }
            Some(pace::PathChange::Opened) => {
                tracing::info!(way, "the way of sending is open again");
                self.metrics.mail_path_opened();
            }
            None => {}
        }
    }
}

/// A way of sending, and the address its messages come from (`mail_from`).
#[derive(Clone)]
struct Sender {
    outbound: Arc<dyn Outbound>,
    from: Mailbox,
}

impl Sender {
    /// The message of `letter`, from this sender; `None` if its recipient's address is
    /// none.
    fn outgoing(&self, letter: Letter) -> Option<Outgoing> {
        Some(Outgoing {
            from: self.from.clone(),
            to: letter.to.parse().ok()?,
            subject: letter.content.subject,
            plain: letter.content.plain,
            html: letter.content.html,
            language: letter.content.language,
        })
    }
}
