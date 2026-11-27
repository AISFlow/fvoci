//! The links mailed with a secret in them: the secret is in the fragment alone.

use std::fmt;

use crate::config::ServerUrl;

/// The pages of the web app that a mailed secret opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretPage {
    /// Choosing a new password.
    ResetPassword,
    /// Confirming the address of a registration.
    VerifyEmail,
    /// Confirming a new address of an account.
    ConfirmEmail,
    /// Taking back the change of an account's address.
    RevertEmail,
}

impl SecretPage {
    fn path(self) -> &'static str {
        match self {
            Self::ResetPassword => "/reset-password",
            Self::VerifyEmail => "/verify-email",
            Self::ConfirmEmail => "/confirm-email",
            Self::RevertEmail => "/revert-email",
        }
    }
}

/// A link to a page of the web app with a secret (`<server_url>/<page>#<secret>`). The
/// secret is in the fragment, which a browser keeps to itself: it never reaches the
/// target of an HTTP request, the log of a proxy or a `Referer` header.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SecretLink {
    page: SecretPage,
    link: String,
}

impl SecretLink {
    /// The link to `page` of the server at `server` with `secret`.
    pub(crate) fn new(server: &ServerUrl, page: SecretPage, secret: &str) -> Self {
        Self {
            page,
            link: server.at(&format!("{}#{secret}", page.path())),
        }
    }

    /// The link, secret and all.
    pub(crate) fn as_str(&self) -> &str {
        &self.link
    }

    /// The link, secret and all, to be handed on.
    pub(crate) fn into_string(self) -> String {
        self.link
    }
}

/// The page alone: the secret is not written to a log.
impl fmt::Debug for SecretLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretLink")
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{SecretLink, SecretPage};
    use crate::config::ServerUrl;

    #[test]
    fn the_secret_is_the_fragment_after_the_page_and_is_not_debugged() {
        let server: ServerUrl = "https://boards.example.com/fvoci/".parse().expect("a URL");
        let link = SecretLink::new(&server, SecretPage::VerifyEmail, "s3cr3t");
        assert_eq!(
            link.as_str(),
            "https://boards.example.com/fvoci/verify-email#s3cr3t"
        );
        assert!(!format!("{link:?}").contains("s3cr3t"), "{link:?}");
    }
}
