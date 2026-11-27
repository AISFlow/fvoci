//! What [`Configuration::check`] read from a configuration it accepted. The settings
//! that are text in the file are kept here as what the check found them to be, and
//! nothing after it reads them from their text again.

use std::num::NonZeroU64;
use std::str::FromStr;
use std::time::Duration;

use anyhow::bail;
use fvoci_store::DbType;

use super::{Configuration, ORG_CREATION_FIRST_USER_ONLY, ORG_CREATION_OPEN, TrustedProxies};
use crate::auth::token::{Origins, origin_of};

/// Whether the server holds one organization or takes in several: `org_creation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgCreation {
    /// `first_user_only`: the server holds one organization, the first user's.
    FirstUserOnly,
    /// `open`: whoever registers starts an organization of their own.
    Open,
}

impl FromStr for OrgCreation {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> anyhow::Result<Self> {
        match text {
            ORG_CREATION_FIRST_USER_ONLY => Ok(Self::FirstUserOnly),
            ORG_CREATION_OPEN => Ok(Self::Open),
            other => bail!(
                "org_creation {other:?} is not known; use {ORG_CREATION_FIRST_USER_ONLY:?} or \
                 {ORG_CREATION_OPEN:?}"
            ),
        }
    }
}

/// The public URL of the server (`server_url`): an `http` or `https` URL without a
/// user, a query or a fragment, and without the spaces around it and the slash it may
/// have been written with at its end, so that a path of the web app is written after it
/// (a link of a mail, whose secret is its own fragment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUrl(String);

impl FromStr for ServerUrl {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        if origin_of(text).is_none() {
            bail!("server_url {text:?} is no http or https URL");
        }
        let after_scheme = text.split_once("://").map_or("", |(_, rest)| rest);
        let authority = after_scheme.split('/').next().unwrap_or_default();
        if authority.contains('@') {
            bail!("server_url {text:?} names a user: the server's links are anybody's");
        }
        if text.contains(['?', '#']) {
            bail!(
                "server_url {text:?} has a query or a fragment, which the paths of the \
                 server's links would follow"
            );
        }
        Ok(Self(text.trim_end_matches('/').to_owned()))
    }
}

impl ServerUrl {
    /// The URL, without a slash at its end.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The address of `path` of the web app, which begins with a slash.
    #[must_use]
    pub fn at(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

/// A configuration that [`Configuration::check`] accepted, with what it read from it.
#[derive(Debug, Clone)]
pub struct Settings {
    config: Configuration,
    org_creation: OrgCreation,
    database_kind: DbType,
    server_url: ServerUrl,
    origins: Origins,
    trusted_proxies: TrustedProxies,
    largest_file: NonZeroU64,
    largest_import: NonZeroU64,
    websocket_ping: Option<Duration>,
}

/// What [`Configuration::check`] found the settings that are text to be.
pub(super) struct Read {
    pub(super) org_creation: OrgCreation,
    pub(super) database_kind: DbType,
    pub(super) server_url: ServerUrl,
    pub(super) origins: Origins,
    pub(super) trusted_proxies: TrustedProxies,
    pub(super) largest_file: NonZeroU64,
    pub(super) largest_import: NonZeroU64,
}

impl Settings {
    /// The settings of `config`, which the check accepted, with what it read.
    pub(super) fn new(config: Configuration, read: Read) -> Self {
        // The test harness turns the pings off where they would only be noise.
        let websocket_ping = (!config.skip_websocket_pings)
            .then(|| Duration::from_secs(config.websocket_ping_seconds));
        Self {
            config,
            org_creation: read.org_creation,
            database_kind: read.database_kind,
            server_url: read.server_url,
            origins: read.origins,
            trusted_proxies: read.trusted_proxies,
            largest_file: read.largest_file,
            largest_import: read.largest_import,
            websocket_ping,
        }
    }

    /// The configuration as it was read.
    #[must_use]
    pub fn config(&self) -> &Configuration {
        &self.config
    }

    /// Whether the server holds one organization or takes in several.
    #[must_use]
    pub fn org_creation(&self) -> OrgCreation {
        self.org_creation
    }

    /// Whether the server takes in several organizations (`org_creation` is `open`).
    #[must_use]
    pub fn takes_several_organizations(&self) -> bool {
        self.org_creation == OrgCreation::Open
    }

    /// The kind of the database.
    #[must_use]
    pub fn database_kind(&self) -> DbType {
        self.database_kind
    }

    /// The public URL of the server.
    #[must_use]
    pub fn server_url(&self) -> &ServerUrl {
        &self.server_url
    }

    /// The origins whose pages may send requests that change something: the server's
    /// own and `allowed_origins`.
    #[must_use]
    pub fn origins(&self) -> &Origins {
        &self.origins
    }

    /// The proxies in front of the server whose word on a client's address is taken.
    #[must_use]
    pub fn trusted_proxies(&self) -> &TrustedProxies {
        &self.trusted_proxies
    }

    /// The largest file an upload takes, in bytes (`max_file_bytes`).
    #[must_use]
    pub fn largest_file(&self) -> NonZeroU64 {
        self.largest_file
    }

    /// The largest archive an import takes, in bytes (`max_import_bytes`).
    #[must_use]
    pub fn largest_import(&self) -> NonZeroU64 {
        self.largest_import
    }

    /// How often the websockets are pinged (`websocket_ping_seconds`); `None` where they
    /// are not.
    #[must_use]
    pub fn websocket_ping(&self) -> Option<Duration> {
        self.websocket_ping
    }
}

#[cfg(test)]
mod tests {
    use super::ServerUrl;

    /// A path of the web app is written after the URL as it was given, without the
    /// spaces around it and its last slash.
    #[test]
    fn a_server_url_is_written_before_the_paths_of_the_web_app() {
        for (given, written) in [
            ("https://fvoci.example", "https://fvoci.example/reset"),
            ("  https://fvoci.example/  ", "https://fvoci.example/reset"),
            (
                "http://localhost:8000/app/",
                "http://localhost:8000/app/reset",
            ),
        ] {
            let url: ServerUrl = given.parse().expect("a server URL");
            assert_eq!(url.at("/reset"), written, "{given:?}");
        }
    }

    /// A URL whose links would not reach the web app's pages is refused: one of another
    /// scheme, and one with a user, a query or a fragment, which the paths of the links
    /// would follow.
    #[test]
    fn a_server_url_with_a_user_a_query_or_a_fragment_is_refused() {
        for given in [
            "",
            "fvoci.example",
            "ftp://fvoci.example",
            "https://admin:secret@fvoci.example",
            "https://admin@fvoci.example/",
            "https://fvoci.example/?lang=ko",
            "https://fvoci.example#top",
        ] {
            assert!(given.parse::<ServerUrl>().is_err(), "{given:?}");
        }
    }
}
