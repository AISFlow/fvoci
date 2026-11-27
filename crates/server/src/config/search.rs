//! The search server what cards and pages say is found through (`search_*`): where it is,
//! how the server signs in to it, and the index that is the server's there. Without
//! `search_url` the database is searched. The secrets never appear in diagnostics.

use std::fmt;
use std::time::Duration;

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::mail::loopback_address;

pub(super) const ENV_KEYS: [(&str, super::EnvKind); 7] = [
    ("search_url", super::EnvKind::Text),
    ("search_username", super::EnvKind::Text),
    ("search_password", super::EnvKind::Text),
    ("search_api_key", super::EnvKind::Text),
    ("search_index", super::EnvKind::Text),
    ("search_ca_file", super::EnvKind::Text),
    ("search_requests_per_second", super::EnvKind::Number),
];

/// The keys whose values are secrets: a value of the wrong type is refused without
/// the error repeating it.
const SECRET_KEYS: [&str; 2] = ["search_password", "search_api_key"];

/// How long a search may take ([`SearchSettings::timeout`]).
const SEARCH_TIMEOUT: Duration = Duration::from_secs(3);

/// The longest name an index may have here; the search servers take up to 255 bytes.
const MAX_INDEX_NAME: usize = 100;

/// The keys of the search server, flattened into the main configuration file.
#[derive(Clone, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct SearchSettings {
    /// The scheme, host and port of the search server; empty for searching the
    /// database.
    pub search_url: String,
    /// The user the server signs in as (HTTP Basic), with `search_password`.
    pub search_username: String,
    /// That user's password; omitted from debug output.
    pub search_password: String,
    /// An Elasticsearch API key, in its encoded form, in place of a user and a
    /// password; omitted from debug output.
    pub search_api_key: String,
    /// The index that is this server's, which it makes, and makes again.
    pub search_index: String,
    /// Optional PEM file of additional root certificates trusted exclusively by the
    /// search HTTP client (`FVOCI_SEARCH_CA_FILE`), keeping search TLS trust isolated
    /// from OIDC, webhooks, S3, and mail.
    pub search_ca_file: String,
    /// How many searches a user may make in a second; 0 for no limit.
    pub search_requests_per_second: u32,
    /// Whether the upkeep of the search runs only when it is called
    /// (`search::tend`), and no task runs it. Not a configuration file key: the tests
    /// set it, to say when what changed is sent.
    #[serde(skip)]
    pub upkeep_by_hand: bool,
    /// How long a search may take, on the search server or in the database: three
    /// seconds. Not a configuration file key: the tests shorten it.
    #[serde(skip)]
    pub timeout: Duration,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            search_url: String::new(),
            search_username: String::new(),
            search_password: String::new(),
            search_api_key: String::new(),
            search_index: "fvoci".to_owned(),
            search_ca_file: String::new(),
            search_requests_per_second: 5,
            upkeep_by_hand: false,
            timeout: SEARCH_TIMEOUT,
        }
    }
}

impl fmt::Debug for SearchSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchSettings")
            .field("search_url", &shown_url(&self.search_url))
            .field("search_username", &self.search_username)
            .field("search_index", &self.search_index)
            .field("search_ca_file", &self.search_ca_file)
            .field(
                "search_requests_per_second",
                &self.search_requests_per_second,
            )
            .finish_non_exhaustive()
    }
}

/// How the server signs in to the search server.
#[derive(Clone, PartialEq, Eq)]
pub enum SearchCredentials {
    /// It does not: the search server takes anybody who reaches it.
    None,
    /// HTTP Basic, as both search servers take it.
    Basic {
        /// The user.
        username: String,
        /// The user's password.
        password: String,
    },
    /// `Authorization: ApiKey`, which Elasticsearch takes.
    ApiKey(String),
}

impl fmt::Debug for SearchCredentials {
    /// Names the way alone, never a user or a secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Basic { .. } => "Basic",
            Self::ApiKey(_) => "ApiKey",
        })
    }
}

/// The search server as the keys name it, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchServer {
    /// `scheme://host[:port]`, without a trailing slash.
    pub url: String,
    /// How the server signs in.
    pub credentials: SearchCredentials,
    /// The index that is this server's.
    pub index: String,
    /// Optional PEM file of root certificates trusted only for this search server.
    pub ca_file: Option<String>,
}

impl SearchSettings {
    /// Whether the keys name a search server.
    pub fn enabled(&self) -> bool {
        !self.search_url.is_empty()
    }

    /// The search server the keys name; `None` without `search_url`.
    ///
    /// # Errors
    ///
    /// Returns an error naming the key, never a secret's value, if the address is not
    /// `https://host[:port]` (or plain `http` to a loopback address), if both ways of
    /// signing in are given or one of a user and a password is missing, if the index
    /// has a name the search servers do not take, or if a key of the search server is
    /// set without `search_url`.
    pub fn server(&self) -> anyhow::Result<Option<SearchServer>> {
        if !self.enabled() {
            let set = [
                ("search_username", &self.search_username),
                ("search_password", &self.search_password),
                ("search_api_key", &self.search_api_key),
                ("search_ca_file", &self.search_ca_file),
            ]
            .into_iter()
            .find(|(_, value)| !value.is_empty());
            if let Some((key, _)) = set {
                bail!("{key} is set, but search_url is not");
            }
            return Ok(None);
        }
        let ca_file =
            (!self.search_ca_file.trim().is_empty()).then(|| self.search_ca_file.trim().to_owned());
        Ok(Some(SearchServer {
            url: self.url()?,
            credentials: self.credentials()?,
            index: self.index()?,
            ca_file,
        }))
    }

    /// `search_url`, checked as `mail_api_endpoint` is: the requests carry the
    /// server's credentials and what cards and pages say, so plain `http` goes to a loopback
    /// address only, written as one.
    fn url(&self) -> anyhow::Result<String> {
        let refused = || {
            anyhow!(
                "search_url must be https://host[:port] (http only to a loopback address \
                 such as 127.0.0.1), without a path, a query or credentials"
            )
        };
        let url = reqwest::Url::parse(&self.search_url).map_err(|_| refused())?;
        let plain_to_here = url.scheme() == "http" && loopback_address(&url).is_some();
        if !(url.scheme() == "https" || plain_to_here)
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.host_str().is_none()
        {
            return Err(refused());
        }
        Ok(url.as_str().trim_end_matches('/').to_owned())
    }

    fn credentials(&self) -> anyhow::Result<SearchCredentials> {
        let basic = !self.search_username.is_empty() || !self.search_password.is_empty();
        if basic && !self.search_api_key.is_empty() {
            bail!("search_api_key is set together with search_username or search_password");
        }
        // The key goes into a header as it is: printable ASCII. Said without the
        // character, as of every secret.
        if !self.search_api_key.chars().all(|c| c.is_ascii_graphic()) {
            bail!("search_api_key has a character it cannot have");
        }
        // A user and a password are sent encoded: anything but a control character.
        if self.search_password.chars().any(char::is_control) {
            bail!("search_password has a character it cannot have");
        }
        if !self.search_api_key.is_empty() {
            return Ok(SearchCredentials::ApiKey(self.search_api_key.clone()));
        }
        if !basic {
            return Ok(SearchCredentials::None);
        }
        if self.search_username.is_empty() || self.search_password.is_empty() {
            bail!("search_username and search_password are given together");
        }
        if self.search_username.contains(':') || self.search_username.chars().any(char::is_control)
        {
            bail!("search_username has a character it cannot have");
        }
        Ok(SearchCredentials::Basic {
            username: self.search_username.clone(),
            password: self.search_password.clone(),
        })
    }

    /// `search_index`, which becomes a part of the path of every request: lower-case
    /// letters, digits, `-` and `_`, beginning with a letter or a digit.
    fn index(&self) -> anyhow::Result<String> {
        let name = &self.search_index;
        let shaped = name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !shaped || name.len() > MAX_INDEX_NAME {
            bail!(
                "search_index must be at most {MAX_INDEX_NAME} lower-case letters, digits, \
                 '-' and '_', beginning with a letter or a digit"
            );
        }
        Ok(name.clone())
    }
}

/// `search_url` as `Debug` shows it: not at all if it carries credentials, which
/// [`SearchSettings::server`] refuses only later.
fn shown_url(url: &str) -> &str {
    match reqwest::Url::parse(url) {
        Ok(parsed) if !parsed.username().is_empty() || parsed.password().is_some() => "<hidden>",
        _ if url.contains('@') => "<hidden>",
        _ => url,
    }
}

/// Checks that the secrets, if the file has them, are text: the error of a value of
/// another type would repeat it.
pub(super) fn check_secrets_are_text(keys: &Map<String, Value>) -> anyhow::Result<()> {
    for key in SECRET_KEYS {
        match keys.get(key) {
            None | Some(Value::String(_)) => {}
            Some(_) => bail!("{key} must be a string"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(url: &str) -> SearchSettings {
        SearchSettings {
            search_url: url.to_owned(),
            ..SearchSettings::default()
        }
    }

    #[test]
    fn without_an_address_there_is_no_search_and_no_other_key() {
        assert_eq!(
            SearchSettings::default().server().expect("checked"),
            None,
            "nothing is set"
        );
        let alone = SearchSettings {
            search_api_key: "secret-key".to_owned(),
            ..SearchSettings::default()
        };
        let error = alone.server().expect_err("a key without the address");
        assert!(error.to_string().contains("search_api_key"), "{error}");
        assert!(!error.to_string().contains("secret-key"), "{error}");
    }

    #[test]
    fn the_address_is_https_or_plain_to_this_machine_and_names_a_host_only() {
        let url = |value: &str| {
            settings(value)
                .server()
                .map(|server| server.expect("a server").url)
                .map_err(|error| error.to_string())
        };
        assert_eq!(
            url("https://search.example.test:9243/"),
            Ok("https://search.example.test:9243".to_owned())
        );
        assert_eq!(
            url("http://127.0.0.1:9200"),
            Ok("http://127.0.0.1:9200".to_owned())
        );
        assert_eq!(url("http://[::1]:9200"), Ok("http://[::1]:9200".to_owned()));
        for refused in [
            "http://search.example.test:9200",
            "http://localhost:9200",
            "https://user:secret-pass@search.example.test",
            "https://search.example.test/prefix",
            "https://search.example.test/?pretty",
            "ftp://127.0.0.1",
            "search.example.test",
        ] {
            let error = url(refused).expect_err(refused);
            assert!(error.contains("search_url"), "{error}");
            assert!(!error.contains("secret-pass"), "{error}");
        }
    }

    #[test]
    fn one_way_of_signing_in_is_given_whole() {
        let with = |username: &str, password: &str, api_key: &str| {
            SearchSettings {
                search_username: username.to_owned(),
                search_password: password.to_owned(),
                search_api_key: api_key.to_owned(),
                ..settings("https://search.example.test")
            }
            .server()
            .map(|server| server.expect("a server").credentials)
            .map_err(|error| error.to_string())
        };
        assert_eq!(with("", "", ""), Ok(SearchCredentials::None));
        assert_eq!(
            with("fvoci", "secret-pass", ""),
            Ok(SearchCredentials::Basic {
                username: "fvoci".to_owned(),
                password: "secret-pass".to_owned(),
            })
        );
        assert_eq!(
            with("", "", "secret-key"),
            Ok(SearchCredentials::ApiKey("secret-key".to_owned()))
        );
        with("검색", "secret pass 비밀", "").expect("any text but control characters");
        for (username, password, api_key) in [
            ("fvoci", "", ""),
            ("", "secret-pass", ""),
            ("fvoci", "secret-pass", "secret-key"),
            ("fv:oci", "secret-pass", ""),
            ("fvoci", "secret\tpass", ""),
            ("fv\noci", "secret-pass", ""),
            ("", "", "secret-key\nX-Other: 1"),
        ] {
            let error = with(username, password, api_key).expect_err("refused");
            assert!(error.contains("search_"), "{error}");
            assert!(!error.contains("secret"), "{error}");
        }
    }

    #[test]
    fn the_index_has_a_name_that_is_a_part_of_a_path_and_nothing_else() {
        let index = |name: &str| {
            SearchSettings {
                search_index: name.to_owned(),
                ..settings("https://search.example.test")
            }
            .server()
            .map(|server| server.expect("a server").index)
        };
        assert_eq!(index("fvoci").expect("the default"), "fvoci");
        assert_eq!(index("fvoci-prod_2").expect("taken"), "fvoci-prod_2");
        for refused in [
            "",
            "_all",
            "-fvoci",
            "Fvoci",
            "fvoci/_search",
            "fvoci,other",
            "fvoci*",
            "fvoci other",
            &"x".repeat(MAX_INDEX_NAME + 1),
        ] {
            let error = index(refused).expect_err(refused);
            assert!(error.to_string().contains("search_index"), "{error}");
        }
    }

    #[test]
    fn nothing_shown_of_the_settings_holds_a_secret() {
        let shown = format!(
            "{:?}",
            SearchSettings {
                search_url: "https://user:secret-in-url@search.example.test".to_owned(),
                search_username: "fvoci".to_owned(),
                search_password: "secret-pass".to_owned(),
                search_api_key: "secret-key".to_owned(),
                ..SearchSettings::default()
            }
        );
        assert!(!shown.contains("secret"), "{shown}");
        let credentials = format!(
            "{:?} {:?}",
            SearchCredentials::Basic {
                username: "fvoci".to_owned(),
                password: "secret-pass".to_owned(),
            },
            SearchCredentials::ApiKey("secret-key".to_owned())
        );
        assert_eq!(credentials, "Basic ApiKey");
    }

    #[test]
    fn a_secret_of_another_type_is_refused_without_its_value() {
        let mut keys = Map::new();
        keys.insert("search_password".to_owned(), Value::from(12_345_678));
        let error = check_secrets_are_text(&keys).expect_err("a number");
        assert_eq!(error.to_string(), "search_password must be a string");
    }
}
