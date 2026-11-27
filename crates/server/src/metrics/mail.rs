//! The metrics of outgoing mail: the messages given up, by kind and by why, and whether
//! the way of sending is closed. No label names an address, an account or a reply of
//! the mail provider.

use prometheus::{IntCounter, IntCounterVec, IntGauge, Opts, Registry};

use super::{Metrics, PREFIX};

/// What a message is: it decides its queue, its share and its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailKind {
    PasswordReset,
    Invitation,
    /// The link that confirms the address of a registration.
    EmailVerification,
    /// What an account's change of its address mails: the link that confirms it, to
    /// the new address, and the notice of it, to the one before.
    EmailChange,
}

impl MailKind {
    const ALL: [Self; 4] = [
        Self::PasswordReset,
        Self::Invitation,
        Self::EmailVerification,
        Self::EmailChange,
    ];

    /// The value of the `kind` label.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PasswordReset => "password_reset",
            Self::Invitation => "invitation",
            Self::EmailVerification => "email_verification",
            Self::EmailChange => "email_change",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::PasswordReset => 0,
            Self::Invitation => 1,
            Self::EmailVerification => 2,
            Self::EmailChange => 3,
        }
    }

    /// Whether the message lets somebody into their own account: it goes before the
    /// invitations, and gives up a closed way as a reset link does.
    pub(crate) fn goes_first(self) -> bool {
        match self {
            Self::PasswordReset | Self::EmailVerification | Self::EmailChange => true,
            Self::Invitation => false,
        }
    }
}

/// Why a message was given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailFailure {
    /// The way of sending refused it for good.
    Permanent,
    /// The way asked for later at every attempt.
    Temporary,
}

impl MailFailure {
    const ALL: [Self; 2] = [Self::Permanent, Self::Temporary];

    /// The value of the `class` label.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Temporary => "temporary",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Permanent => 0,
            Self::Temporary => 1,
        }
    }
}

/// The metrics of outgoing mail.
#[derive(Debug)]
pub(super) struct MailMetrics {
    /// By [`MailKind::index`], then by [`MailFailure::index`].
    failed: [[IntCounter; 2]; 4],
    path_down: IntGauge,
}

impl MailMetrics {
    /// Makes the metrics and registers them in `registry`, every count at zero.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let failed = IntCounterVec::new(
            Opts::new(
                "mail_failed_total",
                "How many messages were given up, by kind (password_reset, invitation, \
                 email_verification, email_change) \
                 and class: permanent (the mail server or provider refused it for good) \
                 or temporary (it asked to try later at every attempt).",
            )
            .namespace(PREFIX),
            &["kind", "class"],
        )?;
        registry.register(Box::new(failed.clone()))?;
        let failed = MailKind::ALL.map(|kind| {
            MailFailure::ALL.map(|class| {
                let counter = failed.with_label_values(&[kind.as_str(), class.as_str()]);
                counter.reset();
                counter
            })
        });
        let path_down = IntGauge::with_opts(
            Opts::new(
                "mail_path_down",
                "1 while the way of sending is closed to this server (it refuses the \
                 server's keys, token or permissions, or cannot be reached): \
                 invitations wait and one message is tried a minute; 0 otherwise.",
            )
            .namespace(PREFIX),
        )?;
        registry.register(Box::new(path_down.clone()))?;
        Ok(Self { failed, path_down })
    }
}

impl Metrics {
    /// Counts a message of `kind` that was given up, for `class`.
    pub(crate) fn mail_failed(&self, kind: MailKind, class: MailFailure) {
        if let Some(kept) = &self.kept {
            kept.mail.failed[kind.index()][class.index()].inc();
        }
    }

    /// Records that the way of sending closed.
    pub(crate) fn mail_path_closed(&self) {
        if let Some(kept) = &self.kept {
            kept.mail.path_down.set(1);
        }
    }

    /// Records that the way of sending opened again.
    pub(crate) fn mail_path_opened(&self) {
        if let Some(kept) = &self.kept {
            kept.mail.path_down.set(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_and_class_has_its_own_counter() {
        for (place, kind) in MailKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), place, "{kind:?}");
        }
        for (place, class) in MailFailure::ALL.into_iter().enumerate() {
            assert_eq!(class.index(), place, "{class:?}");
        }
    }
}
