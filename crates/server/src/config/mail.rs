//! Which way mail leaves by (`mail_transport`), and the keys of the mail providers'
//! HTTP APIs. Secrets never appear in diagnostics, and a key of a way that is not the
//! chosen one is refused rather than passed over.

use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{anyhow, bail};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::smtp::SmtpSettings;

pub(super) const ENV_KEYS: [(&str, super::EnvKind); 14] = [
    ("email_verification", super::EnvKind::Flag),
    ("mail_transport", super::EnvKind::Text),
    ("mail_api_endpoint", super::EnvKind::Text),
    ("cloudflare_account_id", super::EnvKind::Text),
    ("cloudflare_api_token", super::EnvKind::Text),
    ("ses_region", super::EnvKind::Text),
    ("ses_access_key_id", super::EnvKind::Text),
    ("ses_secret_access_key", super::EnvKind::Text),
    ("oci_region", super::EnvKind::Text),
    ("oci_tenancy", super::EnvKind::Text),
    ("oci_user", super::EnvKind::Text),
    ("oci_fingerprint", super::EnvKind::Text),
    ("oci_private_key_file", super::EnvKind::Text),
    ("oci_compartment", super::EnvKind::Text),
];

/// The keys whose values are secrets: a value of the wrong type is refused without
/// the error repeating it.
const SECRET_KEYS: [&str; 2] = ["cloudflare_api_token", "ses_secret_access_key"];

/// The way mail leaves the server; a value that is none of these is refused when the
/// configuration is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "lowercase")]
pub enum MailTransport {
    /// An SMTP relay (`smtp_*`), for servers installed by their users.
    Smtp,
    /// Cloudflare Email Service's REST API (`cloudflare_*`).
    Cloudflare,
    /// Amazon SES's API v2 (`ses_*`).
    Ses,
    /// OCI Email Delivery's HTTPS submission (`oci_*`).
    Oci,
}

impl MailTransport {
    /// The value of `mail_transport` that names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smtp => "smtp",
            Self::Cloudflare => "cloudflare",
            Self::Ses => "ses",
            Self::Oci => "oci",
        }
    }
}

/// The keys of the way of sending, flattened into the main configuration file.
#[derive(Clone, Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct MailSettings {
    /// Whether a registration opens nothing until the link mailed to its address
    /// confirms it; needs a way of sending.
    pub email_verification: bool,
    /// `smtp`, `cloudflare`, `ses` or `oci`; empty for SMTP when `smtp_host` is set and
    /// no mail otherwise.
    #[serde(deserialize_with = "transport_or_none")]
    #[cfg_attr(test, serde(serialize_with = "transport_as_text"))]
    pub mail_transport: Option<MailTransport>,
    /// The scheme, host and port that the provider's requests go to instead of its own
    /// (a fake provider of the tests, another realm); empty for the provider's.
    pub mail_api_endpoint: String,
    /// The Cloudflare account whose Email Service sends.
    pub cloudflare_account_id: String,
    /// The API token; omitted from debug output.
    pub cloudflare_api_token: String,
    /// The AWS region whose SES sends (`us-east-1`).
    pub ses_region: String,
    /// The access key of the IAM user or role that may `ses:SendEmail`.
    pub ses_access_key_id: String,
    /// Its secret access key; omitted from debug output.
    pub ses_secret_access_key: String,
    /// The OCI region whose Email Delivery sends (`us-ashburn-1`).
    pub oci_region: String,
    /// The OCID of the tenancy.
    pub oci_tenancy: String,
    /// The OCID of the user whose API key signs.
    pub oci_user: String,
    /// The fingerprint of that API key (`12:34:…`).
    pub oci_fingerprint: String,
    /// The file of the API key's private half (PEM, unencrypted), read at the start;
    /// its contents are never shown.
    pub oci_private_key_file: String,
    /// The OCID of the compartment of the approved sender.
    pub oci_compartment: String,
}

impl fmt::Debug for MailSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailSettings")
            .field("email_verification", &self.email_verification)
            .field("mail_transport", &self.mail_transport)
            .field(
                "mail_api_endpoint",
                &shown_endpoint(&self.mail_api_endpoint),
            )
            .field("cloudflare_account_id", &self.cloudflare_account_id)
            .field("ses_region", &self.ses_region)
            .field("ses_access_key_id", &self.ses_access_key_id)
            .field("oci_region", &self.oci_region)
            .field("oci_tenancy", &self.oci_tenancy)
            .field("oci_user", &self.oci_user)
            .field("oci_fingerprint", &self.oci_fingerprint)
            .field("oci_private_key_file", &self.oci_private_key_file)
            .field("oci_compartment", &self.oci_compartment)
            .finish_non_exhaustive()
    }
}

fn transport_or_none<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MailTransport>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        return Ok(None);
    }
    MailTransport::deserialize(serde::de::value::StrDeserializer::<D::Error>::new(&text)).map(Some)
}

#[cfg(test)]
#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's serialize_with hands the field by reference"
)]
fn transport_as_text<S: serde::Serializer>(
    transport: &Option<MailTransport>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(transport.map_or("", MailTransport::as_str))
}

/// A secret of the configuration: shown as `<hidden>` by `Debug`.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// The secret itself, for the request it signs or authorizes.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<hidden>")
    }
}

/// What Cloudflare Email Service needs.
#[derive(Debug, Clone)]
pub struct CloudflareKeys {
    /// Where the requests go: `https://api.cloudflare.com`, or `mail_api_endpoint`.
    pub endpoint: String,
    /// `cloudflare_account_id`.
    pub account_id: String,
    /// `cloudflare_api_token`.
    pub api_token: Secret,
}

/// What Amazon SES needs.
#[derive(Debug, Clone)]
pub struct SesKeys {
    /// Where the requests go: `https://email.{region}.amazonaws.com`, or
    /// `mail_api_endpoint`.
    pub endpoint: String,
    /// `ses_region`, which the signature names too.
    pub region: String,
    /// `ses_access_key_id`.
    pub access_key_id: String,
    /// `ses_secret_access_key`.
    pub secret_access_key: Secret,
}

/// What OCI Email Delivery needs; the key is read from its file when the server starts.
#[derive(Debug, Clone)]
pub struct OciKeys {
    /// Where the requests go:
    /// `https://cell0.submit.email.{region}.oci.oraclecloud.com`, or
    /// `mail_api_endpoint`.
    pub endpoint: String,
    /// `oci_tenancy`.
    pub tenancy: String,
    /// `oci_user`.
    pub user: String,
    /// `oci_fingerprint`.
    pub fingerprint: String,
    /// `oci_private_key_file`.
    pub private_key_file: PathBuf,
    /// `oci_compartment`.
    pub compartment: String,
}

/// The way mail leaves, with what it needs, as [`MailSettings::way`] reads it.
#[derive(Debug, Clone)]
pub enum Way {
    /// The SMTP relay of `smtp_*`.
    Smtp,
    /// Cloudflare Email Service.
    Cloudflare(CloudflareKeys),
    /// Amazon SES.
    Ses(SesKeys),
    /// OCI Email Delivery.
    Oci(OciKeys),
}

impl MailSettings {
    /// Whether mail is sent at all: a transport is named, or `smtp_host` is set.
    pub fn enabled(&self, smtp: &SmtpSettings) -> bool {
        self.mail_transport.is_some() || smtp.enabled()
    }

    /// Each provider's keys, with the transport that reads them and their values.
    fn provider_keys(&self) -> [(MailTransport, &'static str, &str); 11] {
        [
            (
                MailTransport::Cloudflare,
                "cloudflare_account_id",
                &self.cloudflare_account_id,
            ),
            (
                MailTransport::Cloudflare,
                "cloudflare_api_token",
                &self.cloudflare_api_token,
            ),
            (MailTransport::Ses, "ses_region", &self.ses_region),
            (
                MailTransport::Ses,
                "ses_access_key_id",
                &self.ses_access_key_id,
            ),
            (
                MailTransport::Ses,
                "ses_secret_access_key",
                &self.ses_secret_access_key,
            ),
            (MailTransport::Oci, "oci_region", &self.oci_region),
            (MailTransport::Oci, "oci_tenancy", &self.oci_tenancy),
            (MailTransport::Oci, "oci_user", &self.oci_user),
            (MailTransport::Oci, "oci_fingerprint", &self.oci_fingerprint),
            (
                MailTransport::Oci,
                "oci_private_key_file",
                &self.oci_private_key_file,
            ),
            (MailTransport::Oci, "oci_compartment", &self.oci_compartment),
        ]
    }

    /// Checks the way mail leaves ([`MailSettings::way`]), and that a registration that
    /// is to be confirmed by mail has one.
    ///
    /// # Errors
    /// Returns the error of [`MailSettings::way`], or one naming `email_verification`
    /// when it is on and no mail is sent.
    pub fn check(&self, smtp: &SmtpSettings) -> anyhow::Result<()> {
        if self.way(smtp)?.is_none() && self.email_verification {
            bail!(
                "email_verification is on, but this server sends no mail: set mail_transport \
                 or smtp_host (FVOCI_MAIL_TRANSPORT, FVOCI_SMTP_HOST), or turn \
                 email_verification (FVOCI_EMAIL_VERIFICATION) off"
            );
        }
        Ok(())
    }

    /// The way mail leaves, or `None` for no mail, with the sender (`mail_from`) checked
    /// for every way. Contacts nothing and reads no file.
    ///
    /// # Errors
    /// Returns an error naming the key that is missing, malformed, or set for a way that
    /// is not the chosen one; never a secret's value.
    pub fn way(&self, smtp: &SmtpSettings) -> anyhow::Result<Option<Way>> {
        let transport = match self.mail_transport {
            Some(transport) => transport,
            None if smtp.enabled() => MailTransport::Smtp,
            None => {
                self.refuse_unused(None)?;
                if !smtp.mail_from.is_empty() || !smtp.smtp_user.is_empty() {
                    bail!(
                        "smtp_host or mail_transport is required when mail settings are supplied"
                    );
                }
                return Ok(None);
            }
        };
        self.refuse_unused(Some(transport))?;
        if !fvoci_model::is_email_valid(&smtp.mail_from) {
            bail!("mail_from must be a single email address: the sender of all mail");
        }
        if transport != MailTransport::Smtp {
            refuse_relay(smtp, transport)?;
        }
        Ok(Some(match transport {
            MailTransport::Smtp => {
                if !smtp.enabled() {
                    bail!("mail_transport is \"smtp\", which needs smtp_host");
                }
                Way::Smtp
            }
            MailTransport::Cloudflare => self.cloudflare()?,
            MailTransport::Ses => self.ses()?,
            MailTransport::Oci => self.oci()?,
        }))
    }

    /// Cloudflare's way, from its keys.
    fn cloudflare(&self) -> anyhow::Result<Way> {
        let account_id = needed(
            MailTransport::Cloudflare,
            "cloudflare_account_id",
            &self.cloudflare_account_id,
        )?;
        let api_token = needed(
            MailTransport::Cloudflare,
            "cloudflare_api_token",
            &self.cloudflare_api_token,
        )?;
        shaped("cloudflare_account_id", account_id, |c| {
            c.is_ascii_alphanumeric()
        })?;
        secret_shaped("cloudflare_api_token", api_token)?;
        Ok(Way::Cloudflare(CloudflareKeys {
            endpoint: self.endpoint("https://api.cloudflare.com".to_owned())?,
            account_id: account_id.to_owned(),
            api_token: Secret(api_token.to_owned()),
        }))
    }

    /// SES's way, from its keys.
    fn ses(&self) -> anyhow::Result<Way> {
        let region = needed(MailTransport::Ses, "ses_region", &self.ses_region)?;
        let access_key_id = needed(
            MailTransport::Ses,
            "ses_access_key_id",
            &self.ses_access_key_id,
        )?;
        let secret_access_key = needed(
            MailTransport::Ses,
            "ses_secret_access_key",
            &self.ses_secret_access_key,
        )?;
        shaped("ses_region", region, is_region_char)?;
        shaped("ses_access_key_id", access_key_id, |c| {
            c.is_ascii_alphanumeric()
        })?;
        secret_shaped("ses_secret_access_key", secret_access_key)?;
        // A temporary key is good only with a session token, which this server
        // does not send: every request would be refused.
        if access_key_id.starts_with("ASIA") {
            bail!(
                "ses_access_key_id is a temporary key (ASIA…), which needs a session \
                 token; give the long-term key of an IAM user (AKIA…)"
            );
        }
        Ok(Way::Ses(SesKeys {
            endpoint: self.endpoint(format!("https://email.{region}.amazonaws.com"))?,
            region: region.to_owned(),
            access_key_id: access_key_id.to_owned(),
            secret_access_key: Secret(secret_access_key.to_owned()),
        }))
    }

    /// OCI's way, from its keys.
    fn oci(&self) -> anyhow::Result<Way> {
        let region = needed(MailTransport::Oci, "oci_region", &self.oci_region)?;
        let tenancy = needed(MailTransport::Oci, "oci_tenancy", &self.oci_tenancy)?;
        let user = needed(MailTransport::Oci, "oci_user", &self.oci_user)?;
        let fingerprint = needed(MailTransport::Oci, "oci_fingerprint", &self.oci_fingerprint)?;
        let private_key_file = needed(
            MailTransport::Oci,
            "oci_private_key_file",
            &self.oci_private_key_file,
        )?;
        let compartment = needed(MailTransport::Oci, "oci_compartment", &self.oci_compartment)?;
        shaped("oci_region", region, is_region_char)?;
        for (key, value) in [
            ("oci_tenancy", tenancy),
            ("oci_user", user),
            ("oci_compartment", compartment),
        ] {
            shaped(key, value, is_ocid_char)?;
        }
        // The console's form: the 16 bytes of the key's MD5, `xx:` in lower case.
        let pairs: Vec<&str> = fingerprint.split(':').collect();
        if pairs.len() != 16
            || !pairs.iter().all(|pair| {
                pair.len() == 2
                    && pair
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
        {
            bail!("oci_fingerprint must be 16 lower-case hexadecimal pairs joined by `:`");
        }
        Ok(Way::Oci(OciKeys {
            endpoint: self.endpoint(format!(
                "https://cell0.submit.email.{region}.oci.oraclecloud.com"
            ))?,
            tenancy: tenancy.to_owned(),
            user: user.to_owned(),
            fingerprint: fingerprint.to_owned(),
            private_key_file: PathBuf::from(private_key_file),
            compartment: compartment.to_owned(),
        }))
    }

    /// Refuses a provider's key, or the endpoint, set for a way that does not read it.
    fn refuse_unused(&self, transport: Option<MailTransport>) -> anyhow::Result<()> {
        let chosen = transport.map_or("not set", MailTransport::as_str);
        if let Some((_, key, _)) = self
            .provider_keys()
            .into_iter()
            .find(|(provider, _, value)| Some(*provider) != transport && !value.is_empty())
        {
            bail!("{key} is set, but mail_transport is {chosen:?}");
        }
        if !self.mail_api_endpoint.is_empty()
            && matches!(transport, None | Some(MailTransport::Smtp))
        {
            bail!("mail_api_endpoint is set, but mail_transport is {chosen:?}");
        }
        Ok(())
    }

    /// `mail_api_endpoint` when it is set, checked, or the provider's own `default`.
    /// Plain `http` goes to a loopback address only, written as one: the requests carry
    /// the provider's keys, and a name (`*.localhost`) is resolved by whatever the
    /// system asks.
    fn endpoint(&self, default: String) -> anyhow::Result<String> {
        if self.mail_api_endpoint.is_empty() {
            return Ok(default);
        }
        let refused = || {
            anyhow!(
                "mail_api_endpoint must be https://host[:port] (http only to a loopback \
                 address such as 127.0.0.1), without a path, a query or credentials"
            )
        };
        let url = reqwest::Url::parse(&self.mail_api_endpoint).map_err(|_| refused())?;
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
}

/// `value` of `key`, which `transport` needs.
fn needed<'a>(transport: MailTransport, key: &str, value: &'a str) -> anyhow::Result<&'a str> {
    if value.is_empty() {
        bail!(
            "mail_transport is \"{}\", which needs {key}",
            transport.as_str()
        );
    }
    Ok(value)
}

/// Refuses the relay's keys for a provider's `transport`: they would be passed over.
fn refuse_relay(smtp: &SmtpSettings, transport: MailTransport) -> anyhow::Result<()> {
    if smtp.enabled() || !smtp.smtp_user.is_empty() {
        bail!(
            "smtp_host and smtp_user are for mail_transport \"smtp\", not \"{}\"",
            transport.as_str()
        );
    }
    Ok(())
}

/// `mail_api_endpoint` as `Debug` shows it: not at all if it carries credentials,
/// which [`MailSettings::way`] refuses only later.
fn shown_endpoint(endpoint: &str) -> &str {
    match reqwest::Url::parse(endpoint) {
        Ok(url) if !url.username().is_empty() || url.password().is_some() => "<hidden>",
        _ if endpoint.contains('@') => "<hidden>",
        _ => endpoint,
    }
}

/// Whether `c` may be in the name of a region (`us-east-1`), which becomes a part of a
/// host name.
fn is_region_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// Whether `c` may be in an OCID (`ocid1.tenancy.oc1..aaaa`), which becomes a part of
/// the key's name in a header and of a request's body.
fn is_ocid_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_')
}

/// Refuses a value with a character `allowed` does not take: these become parts of a
/// host name, a path or a header.
fn shaped(key: &str, value: &str, allowed: impl Fn(char) -> bool) -> anyhow::Result<()> {
    if value.chars().all(allowed) {
        Ok(())
    } else {
        bail!("{key} has a character it cannot have")
    }
}

/// Refuses a secret that is not printable ASCII without spaces, without saying which
/// character it was.
fn secret_shaped(key: &str, value: &str) -> anyhow::Result<()> {
    shaped(key, value, |c| c.is_ascii_graphic())
}

/// The address `url` names, if it is written as an address of this machine's
/// loopback (also one mapped into IPv6).
pub(super) fn loopback_address(url: &reqwest::Url) -> Option<IpAddr> {
    // A URL writes an IPv6 address in brackets, and an IPv4 one in its usual form.
    let address = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok()?
        .to_canonical();
    address.is_loopback().then_some(address)
}

/// Whether `url` names this machine: `localhost`, or a loopback or the unspecified
/// address (also one mapped into IPv6).
pub fn is_loopback_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if loopback_address(url).is_some() {
        true
    } else if let Ok(address) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        address.to_canonical().is_unspecified()
    } else {
        // The parser writes the name in lower case; a trailing dot names the same host.
        let name = host.trim_end_matches('.');
        name == "localhost" || name.ends_with(".localhost")
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
    use serde_json::json;

    use super::*;
    use crate::config::Configuration;

    const TOKEN: &str = "private-cloudflare-token-0123456789";

    /// The configuration of `keys`, checked: the error's text, or the way.
    fn checked(keys: &Value) -> Result<Option<Way>, String> {
        let config =
            Configuration::parse(&keys.to_string(), |_| None).map_err(|e| e.to_string())?;
        config.clone().check().map_err(|e| e.to_string())?;
        config.mail.way(&config.smtp).map_err(|e| e.to_string())
    }

    fn cloudflare() -> Value {
        json!({
            "mail_transport": "cloudflare",
            "cloudflare_account_id": "0123456789abcdef0123456789abcdef",
            "cloudflare_api_token": TOKEN,
            "mail_from": "sender@example.test",
        })
    }

    #[test]
    fn the_transport_is_smtp_with_a_host_and_nothing_without() {
        assert!(matches!(checked(&json!({})), Ok(None)));
        let smtp = json!({"smtp_host": "smtp.example.test", "mail_from": "sender@example.test"});
        assert!(matches!(checked(&smtp), Ok(Some(Way::Smtp))));
        let mut named = smtp;
        named["mail_transport"] = Value::from("smtp");
        assert!(matches!(checked(&named), Ok(Some(Way::Smtp))));
        let empty = json!({"mail_transport": ""});
        assert!(matches!(checked(&empty), Ok(None)), "empty is unset");
        let error = checked(&json!({"mail_transport": "smtp", "mail_from": "sender@example.test"}))
            .expect_err("no relay");
        assert!(error.contains("smtp_host"), "{error}");
        let error = checked(&json!({"mail_transport": "postmark"})).expect_err("unknown");
        assert!(
            error.contains("wrong type") || error.contains("unknown variant"),
            "{error}"
        );
        let mut env = BTreeMapEnv::default();
        env.0.push(("FVOCI_MAIL_TRANSPORT", "cloudflare"));
        let config = Configuration::parse("{}", |name| env.get(name)).expect("parse");
        assert_eq!(config.mail.mail_transport, Some(MailTransport::Cloudflare));
    }

    #[test]
    fn email_verification_is_off_by_default_and_needs_a_way_of_sending() {
        let config = Configuration::parse("{}", |_| None).expect("parse");
        assert!(!config.mail.email_verification, "off unless turned on");
        let error = checked(&json!({"email_verification": true})).expect_err("no mail");
        assert!(error.contains("email_verification"), "{error}");
        let mut smtp =
            json!({"smtp_host": "smtp.example.test", "mail_from": "sender@example.test"});
        smtp["email_verification"] = Value::Bool(true);
        assert!(matches!(checked(&smtp), Ok(Some(Way::Smtp))));
        let mut cloudflare = cloudflare();
        cloudflare["email_verification"] = Value::Bool(true);
        assert!(matches!(checked(&cloudflare), Ok(Some(Way::Cloudflare(_)))));
        let mut env = BTreeMapEnv::default();
        env.0.push(("FVOCI_EMAIL_VERIFICATION", "true"));
        let config = Configuration::parse("{}", |name| env.get(name)).expect("parse");
        assert!(config.mail.email_verification, "turned on by its variable");
        assert!(config.check().is_err(), "and refused without mail");
    }

    #[test]
    fn a_provider_without_a_key_it_needs_is_refused_by_the_keys_name() {
        assert!(matches!(
            checked(&cloudflare()),
            Ok(Some(Way::Cloudflare(_)))
        ));
        for key in ["cloudflare_account_id", "cloudflare_api_token", "mail_from"] {
            let mut keys = cloudflare();
            keys.as_object_mut().expect("an object").remove(key);
            let error = checked(&keys).expect_err("a missing key");
            assert!(error.contains(key), "{key}: {error}");
            assert!(!error.contains(TOKEN), "{error}");
        }
    }

    #[test]
    fn keys_the_chosen_way_does_not_read_are_refused() {
        for (key, value) in [("smtp_host", "smtp.example.test"), ("smtp_user", "user")] {
            let mut keys = cloudflare();
            keys[key] = Value::from(value);
            if key == "smtp_user" {
                keys["smtp_password"] = Value::from("password");
            }
            let error = checked(&keys).expect_err("a key of SMTP");
            assert!(error.contains(key), "{key}: {error}");
        }
        let smtp = json!({
            "smtp_host": "smtp.example.test",
            "mail_from": "sender@example.test",
            "cloudflare_api_token": TOKEN,
        });
        let error = checked(&smtp).expect_err("a key of Cloudflare");
        assert!(error.contains("cloudflare_api_token"), "{error}");
        assert!(!error.contains(TOKEN), "{error}");
        let off = json!({"cloudflare_account_id": "abc"});
        let error = checked(&off).expect_err("a key without mail");
        assert!(error.contains("cloudflare_account_id"), "{error}");
        let mut endpoint =
            json!({"smtp_host": "smtp.example.test", "mail_from": "sender@example.test"});
        endpoint["mail_api_endpoint"] = Value::from("http://127.0.0.1:9");
        let error = checked(&endpoint).expect_err("SMTP takes no endpoint");
        assert!(error.contains("mail_api_endpoint"), "{error}");
    }

    #[test]
    fn the_endpoint_is_https_or_plain_to_this_machine_and_names_a_host_only() {
        let endpoint = |value: &str| {
            let mut keys = cloudflare();
            keys["mail_api_endpoint"] = Value::from(value);
            match checked(&keys) {
                Ok(Some(Way::Cloudflare(keys))) => Ok(keys.endpoint),
                Ok(other) => panic!("{other:?}"),
                Err(error) => Err(error),
            }
        };
        assert_eq!(
            endpoint("https://mail.example.test:8443/"),
            Ok("https://mail.example.test:8443".to_owned())
        );
        assert_eq!(
            endpoint("http://127.0.0.1:8025"),
            Ok("http://127.0.0.1:8025".to_owned())
        );
        assert_eq!(
            endpoint("http://[::1]:8025"),
            Ok("http://[::1]:8025".to_owned())
        );
        for refused in [
            "http://mail.example.test",
            "http://localhost:8025",
            "http://mail.localhost:8025",
            "http://0.0.0.0:8025",
            "ftp://127.0.0.1",
            "https://mail.example.test/client/v4",
            "https://mail.example.test/?x=1",
            "https://user:pass@mail.example.test",
            "not a url",
        ] {
            let error = endpoint(refused).expect_err(refused);
            assert!(error.contains("mail_api_endpoint"), "{refused}: {error}");
        }
        let mut keys = cloudflare();
        keys.as_object_mut()
            .expect("an object")
            .remove("mail_api_endpoint");
        let Ok(Some(Way::Cloudflare(keys))) = checked(&keys) else {
            panic!("the provider's own address");
        };
        assert_eq!(keys.endpoint, "https://api.cloudflare.com");
    }

    #[test]
    fn values_that_become_a_path_or_a_header_have_their_shape() {
        for (key, value) in [
            ("cloudflare_account_id", "abc/../def"),
            ("cloudflare_account_id", "abc?x"),
            ("cloudflare_api_token", "token with spaces"),
            ("cloudflare_api_token", "token\r\nX-Other: 1"),
        ] {
            let mut keys = cloudflare();
            keys[key] = Value::from(value);
            let error = checked(&keys).expect_err(value);
            assert!(error.contains(key), "{key}: {error}");
            assert!(!error.contains(value), "the value is not repeated: {error}");
        }
    }

    #[test]
    fn the_token_never_appears_in_debug_or_type_errors() {
        let config = Configuration::parse(&cloudflare().to_string(), |_| None).expect("parse");
        let debug = format!("{config:?}");
        assert!(!debug.contains(TOKEN), "{debug}");
        let Ok(Some(Way::Cloudflare(keys))) = config.mail.way(&config.smtp) else {
            panic!("Cloudflare");
        };
        let debug = format!("{keys:?}");
        assert!(!debug.contains(TOKEN), "{debug}");
        assert_eq!(keys.api_token.expose(), TOKEN);
        let error = Configuration::parse(r#"{"cloudflare_api_token": 1234567890}"#, |_| None)
            .expect_err("a number is no token")
            .to_string();
        assert!(!error.contains("1234567890"), "{error}");
        assert!(error.contains("cloudflare_api_token"), "{error}");
    }

    const SES_SECRET: &str = "private/ses+secret0123456789";

    fn ses() -> Value {
        json!({
            "mail_transport": "ses",
            "ses_region": "eu-west-2",
            "ses_access_key_id": "AKIDEXAMPLE",
            "ses_secret_access_key": SES_SECRET,
            "mail_from": "sender@example.test",
        })
    }

    #[test]
    fn ses_needs_its_region_and_keys_and_names_its_host_by_the_region() {
        let Ok(Some(Way::Ses(keys))) = checked(&ses()) else {
            panic!("SES");
        };
        assert_eq!(keys.endpoint, "https://email.eu-west-2.amazonaws.com");
        assert_eq!(keys.region, "eu-west-2");
        assert_eq!(keys.secret_access_key.expose(), SES_SECRET);
        assert!(!format!("{keys:?}").contains(SES_SECRET));
        for key in ["ses_region", "ses_access_key_id", "ses_secret_access_key"] {
            let mut keys = ses();
            keys.as_object_mut().expect("an object").remove(key);
            let error = checked(&keys).expect_err("a missing key");
            assert!(error.contains(key), "{key}: {error}");
            assert!(!error.contains(SES_SECRET), "{error}");
        }
        for (key, value) in [
            ("ses_region", "eu-west-2.evil.test/"),
            ("ses_region", "EU-WEST-2"),
            ("ses_access_key_id", "AKID/x"),
            ("ses_secret_access_key", "two words"),
        ] {
            let mut keys = ses();
            keys[key] = Value::from(value);
            let error = checked(&keys).expect_err(value);
            assert!(error.contains(key), "{key}: {error}");
            assert!(!error.contains(value), "{error}");
        }
        let mut temporary = ses();
        temporary["ses_access_key_id"] = Value::from("ASIAEXAMPLE");
        let error = checked(&temporary).expect_err("a temporary key");
        assert!(error.contains("ses_access_key_id"), "{error}");
        let mut both = ses();
        both["cloudflare_api_token"] = Value::from(TOKEN);
        let error = checked(&both).expect_err("a key of Cloudflare");
        assert!(error.contains("cloudflare_api_token"), "{error}");
        let config = Configuration::parse(&ses().to_string(), |_| None).expect("parse");
        assert!(!format!("{config:?}").contains(SES_SECRET));
        let error = Configuration::parse(r#"{"ses_secret_access_key": 1234567890}"#, |_| None)
            .expect_err("a number is no secret")
            .to_string();
        assert!(!error.contains("1234567890"), "{error}");
    }

    fn oci() -> Value {
        json!({
            "mail_transport": "oci",
            "oci_region": "ap-seoul-1",
            "oci_tenancy": "ocid1.tenancy.oc1..aaaaaaaaba3pv6wkcr4jqae5f15p2b2m2yt2j6rx32uzr4h25vqstifsfdsq",
            "oci_user": "ocid1.user.oc1..aaaaaaaat5nvwcna5j6aqzjcaty5eqbb6qt2jvpkanghtgdaqedqw3rynjq",
            "oci_fingerprint": "73:61:a2:21:67:e0:df:be:7e:4b:93:1e:15:98:a5:b7",
            "oci_private_key_file": "oci_api_key.pem",
            "oci_compartment": "ocid1.compartment.oc1..aaaaaaaam3we6vgnherjq5q2idnccdflvjsnog7mlr6rtdb25gilchfeyjxa",
            "mail_from": "sender@example.test",
        })
    }

    #[test]
    fn oci_needs_its_keys_and_names_its_host_by_the_region() {
        let Ok(Some(Way::Oci(keys))) = checked(&oci()) else {
            panic!("OCI");
        };
        assert_eq!(
            keys.endpoint,
            "https://cell0.submit.email.ap-seoul-1.oci.oraclecloud.com"
        );
        assert_eq!(keys.private_key_file, PathBuf::from("oci_api_key.pem"));
        for key in [
            "oci_region",
            "oci_tenancy",
            "oci_user",
            "oci_fingerprint",
            "oci_private_key_file",
            "oci_compartment",
        ] {
            let mut keys = oci();
            keys.as_object_mut().expect("an object").remove(key);
            let error = checked(&keys).expect_err("a missing key");
            assert!(error.contains(key), "{key}: {error}");
        }
        for (key, value) in [
            ("oci_region", "ap-seoul-1.evil.test"),
            ("oci_tenancy", "ocid1.tenancy\"/x"),
            ("oci_user", "OCID1.USER"),
            ("oci_compartment", "ocid1.compartment oc1"),
            ("oci_fingerprint", "73:61:zz"),
            (
                "oci_fingerprint",
                "73:61:A2:21:67:E0:DF:BE:7E:4B:93:1E:15:98:A5:B7",
            ),
            ("oci_fingerprint", "73:61:a2"),
            ("oci_fingerprint", ":::::::::::::::"),
        ] {
            let mut keys = oci();
            keys[key] = Value::from(value);
            let error = checked(&keys).expect_err(value);
            assert!(error.contains(key), "{key}: {error}");
        }
        let mut both = oci();
        both["ses_region"] = Value::from("eu-west-2");
        let error = checked(&both).expect_err("a key of SES");
        assert!(error.contains("ses_region"), "{error}");
    }

    /// Environment variables for [`Configuration::parse`].
    #[derive(Default)]
    struct BTreeMapEnv(Vec<(&'static str, &'static str)>);

    impl BTreeMapEnv {
        fn get(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }
}
