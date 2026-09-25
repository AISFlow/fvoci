//! Typed instance settings catalog (source `packages/contracts/src/settings-catalog.ts`).
//!
//! Every key is one JSON document with a strict shape. The same parser checks
//! stored rows (an invalid row falls back to the default with a warning) and
//! admin PATCH input (an invalid value is a 400), so what the form sends and
//! what the server accepts come from one place. Secrets and infrastructure
//! values are environment-only and never appear here.

use std::sync::LazyLock;

use regex::Regex;
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::validate::utf16_len;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    Live,
    RestartRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Admin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    Branding,
    DefaultsUser,
    Auth,
    Share,
    Embed,
    Features,
    AttachmentPreview,
    I18n,
    Security,
    Operator,
}

pub const SETTINGS_KEYS: [SettingsKey; 10] = [
    SettingsKey::Branding,
    SettingsKey::DefaultsUser,
    SettingsKey::Auth,
    SettingsKey::Share,
    SettingsKey::Embed,
    SettingsKey::Features,
    SettingsKey::AttachmentPreview,
    SettingsKey::I18n,
    SettingsKey::Security,
    SettingsKey::Operator,
];

impl SettingsKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Branding => "branding",
            Self::DefaultsUser => "defaults.user",
            Self::Auth => "auth",
            Self::Share => "share",
            Self::Embed => "embed",
            Self::Features => "features",
            Self::AttachmentPreview => "attachmentPreview",
            Self::I18n => "i18n",
            Self::Security => "security",
            Self::Operator => "operator",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        SETTINGS_KEYS.into_iter().find(|k| k.as_str() == key)
    }

    pub fn safety(self) -> Safety {
        match self {
            Self::Embed | Self::Features => Safety::RestartRequired,
            _ => Safety::Live,
        }
    }

    pub fn visibility(self) -> Visibility {
        match self {
            Self::Auth | Self::Embed | Self::I18n | Self::Security => Visibility::Admin,
            _ => Visibility::Public,
        }
    }

    /// Leaf -> environment variable that wins over the stored value.
    pub fn env_fallback(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Features => &[("ai", "FVOCI_AI_ENABLED")],
            _ => &[],
        }
    }
}

/// A settings document: strict JSON shape plus the source's refinements.
/// `normalize` applies zod `.trim()` transforms and rejects values the source
/// schema rejects.
pub trait SettingsDoc: Serialize + DeserializeOwned + Clone {
    fn normalize(self) -> Option<Self>;
}

pub fn parse_doc<T: SettingsDoc>(value: &Value) -> Option<T> {
    serde_json::from_value::<T>(value.clone())
        .ok()
        .and_then(SettingsDoc::normalize)
}

/// `nullable()` without `optional()`: the key must be present, the value may
/// be null. A `deserialize_with` field is required by serde even for `Option`.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn no_control(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// Source `brandText`: trimmed, 1–80 UTF-16 units, no control characters
/// (the value reaches mail `From` headers).
fn brand_text(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    let len = utf16_len(&trimmed);
    (1..=80).contains(&len).then_some(())?;
    no_control(&trimmed).then_some(trimmed)
}

fn operator_text(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    let len = utf16_len(&trimmed);
    (1..=200).contains(&len).then_some(())?;
    no_control(&trimmed).then_some(trimmed)
}

fn opt<F: Fn(String) -> Option<String>>(value: Option<String>, f: F) -> Option<Option<String>> {
    match value {
        None => Some(None),
        Some(v) => f(v).map(Some),
    }
}

static HOSTNAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9.-]+$").expect("hostname regex"));
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    // zod v4 `z.email()` default pattern without its two look-aheads, which
    // `is_zod_email` checks separately (no leading dot, no "..").
    Regex::new(r"^[A-Za-z0-9_'+\-.]*[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$")
        .expect("email regex")
});
static SHA256_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").expect("sha256 regex"));
static MESSAGE_VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{(\w+)\}\}").expect("message var regex"));

pub fn is_zod_email(value: &str) -> bool {
    !value.starts_with('.') && !value.contains("..") && EMAIL_RE.is_match(value)
}

fn hostname(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    let len = utf16_len(&trimmed);
    ((1..=253).contains(&len) && HOSTNAME_RE.is_match(&trimmed)).then_some(trimmed)
}

fn security_contact(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if utf16_len(&trimmed) > 320 || !no_control(&trimmed) || trimmed.is_empty() {
        return None;
    }
    let ok = match trimmed.strip_prefix("mailto:") {
        Some(address) => is_zod_email(address),
        None => trimmed.starts_with("https://") && trimmed.len() > "https://".len(),
    };
    ok.then_some(trimmed)
}

/// Raster formats only: SVG is a script-bearing document and is never served
/// from this same-origin anonymous route. APNG is sniffed separately from PNG.
pub const BRANDING_ASSET_MIME: [&str; 4] = ["image/png", "image/apng", "image/webp", "image/jpeg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrandingAssetKind {
    Logo,
    Favicon,
}

impl BrandingAssetKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "logo" => Some(Self::Logo),
            "favicon" => Some(Self::Favicon),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
        }
    }
}

/// The identity the upload route recorded: storage key, digest, sniffed type.
/// Serving checks the digest, so a hand-written key cannot turn this public
/// route into a reader for another object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct BrandingAsset {
    pub key: Uuid,
    pub sha256: String,
    pub mime: String,
}

impl BrandingAsset {
    fn valid(&self) -> bool {
        SHA256_RE.is_match(&self.sha256) && BRANDING_ASSET_MIME.contains(&self.mime.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct BrandingSettings {
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub smtp_from_display: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub logo: Option<BrandingAsset>,
    #[serde(deserialize_with = "nullable")]
    pub favicon: Option<BrandingAsset>,
    #[serde(deserialize_with = "nullable")]
    pub login_brand_text: Option<String>,
}

impl BrandingSettings {
    pub fn default_named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            smtp_from_display: None,
            logo: None,
            favicon: None,
            login_brand_text: None,
        }
    }

    pub fn asset(&self, kind: BrandingAssetKind) -> Option<&BrandingAsset> {
        match kind {
            BrandingAssetKind::Logo => self.logo.as_ref(),
            BrandingAssetKind::Favicon => self.favicon.as_ref(),
        }
    }

    pub fn set_asset(&mut self, kind: BrandingAssetKind, asset: Option<BrandingAsset>) {
        match kind {
            BrandingAssetKind::Logo => self.logo = asset,
            BrandingAssetKind::Favicon => self.favicon = asset,
        }
    }
}

impl SettingsDoc for BrandingSettings {
    fn normalize(self) -> Option<Self> {
        if self.logo.as_ref().is_some_and(|a| !a.valid())
            || self.favicon.as_ref().is_some_and(|a| !a.valid())
        {
            return None;
        }
        Some(Self {
            name: brand_text(self.name)?,
            smtp_from_display: opt(self.smtp_from_display, brand_text)?,
            logo: self.logo,
            favicon: self.favicon,
            login_brand_text: opt(self.login_brand_text, brand_text)?,
        })
    }
}

/// PATCH shape of `branding`: the asset leaves are absent, so only the upload
/// route can write them (an arbitrary key is a strict-object 400).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandingPatch {
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub smtp_from_display: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub login_brand_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct DefaultsUserSettings {
    pub locale: String,
    pub timezone: String,
    pub week_starts_on: i64,
    pub text_scale: i64,
}

impl Default for DefaultsUserSettings {
    fn default() -> Self {
        Self {
            locale: "ko".to_string(),
            timezone: "Asia/Seoul".to_string(),
            week_starts_on: 1,
            text_scale: 16,
        }
    }
}

impl SettingsDoc for DefaultsUserSettings {
    fn normalize(self) -> Option<Self> {
        let timezone = self.timezone.trim().to_string();
        let ok = self.locale == "ko"
            && (1..=64).contains(&utf16_len(&timezone))
            && matches!(self.week_starts_on, 0 | 1)
            && matches!(self.text_scale, 16 | 18 | 20);
        ok.then_some(Self { timezone, ..self })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct AuthSettings {
    pub password_min_length: i64,
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            password_min_length: 10,
        }
    }
}

impl SettingsDoc for AuthSettings {
    fn normalize(self) -> Option<Self> {
        (10..=128)
            .contains(&self.password_min_length)
            .then_some(self)
    }
}

/// Instance share-link policy. Share-link routes read it through
/// [`crate::settings::share_policy`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct SharePolicy {
    pub enabled: bool,
    pub default_expires_days: i64,
    pub max_expires_days: i64,
}

impl Default for SharePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            default_expires_days: 7,
            max_expires_days: 365,
        }
    }
}

impl SettingsDoc for SharePolicy {
    fn normalize(self) -> Option<Self> {
        let ok = (1..=365).contains(&self.default_expires_days)
            && (1..=365).contains(&self.max_expires_days)
            && self.default_expires_days <= self.max_expires_days;
        ok.then_some(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct EmbedSettings {
    pub hosts: Vec<String>,
}

impl Default for EmbedSettings {
    fn default() -> Self {
        Self {
            hosts: [
                "youtube.com",
                "www.youtube.com",
                "youtu.be",
                "vimeo.com",
                "player.vimeo.com",
                "figma.com",
                "www.figma.com",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }
}

impl SettingsDoc for EmbedSettings {
    fn normalize(self) -> Option<Self> {
        if self.hosts.len() > 100 {
            return None;
        }
        let hosts = self
            .hosts
            .into_iter()
            .map(hostname)
            .collect::<Option<Vec<_>>>()?;
        Some(Self { hosts })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct FeaturesSettings {
    pub ai: bool,
}

impl SettingsDoc for FeaturesSettings {
    fn normalize(self) -> Option<Self> {
        Some(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct AttachmentPreviewSettings {
    pub mode: String,
}

impl Default for AttachmentPreviewSettings {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
        }
    }
}

impl SettingsDoc for AttachmentPreviewSettings {
    fn normalize(self) -> Option<Self> {
        matches!(self.mode.as_str(), "auto" | "client" | "server").then_some(self)
    }
}

/// Server-sent strings an operator may override, with the exact `{{var}}` set
/// each must keep (source `OVERRIDABLE_MESSAGES`).
pub const OVERRIDABLE_MESSAGES: &[(&str, &[&str])] = &[
    ("seed.status.backlog", &[]),
    ("seed.status.todo", &[]),
    ("seed.status.in_progress", &[]),
    ("seed.status.review", &[]),
    ("seed.status.done", &[]),
    ("seed.status.canceled", &[]),
    ("mail.magic.login.subject", &[]),
    ("mail.magic.reset.subject", &[]),
    ("mail.magic.emailChange.subject", &[]),
    ("mail.magic.emailChangeRequested.subject", &[]),
    ("mail.magic.emailChangeCompleted.subject", &[]),
    ("mail.magic.link.text", &["url", "minutes"]),
    ("mail.magic.emailChangeRequested.text", &[]),
    ("mail.magic.emailChangeCompleted.text", &[]),
    ("mail.invite.subject", &[]),
    ("mail.identity.linked.subject", &[]),
    ("mail.identity.linked.text", &["provider"]),
    ("mail.identity.unlinked.subject", &[]),
    ("mail.identity.unlinked.text", &["provider"]),
    ("withdrawn.displayName", &[]),
    ("task.duplicate.suffix", &["title"]),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct I18nSettings {
    pub overrides: std::collections::BTreeMap<String, String>,
}

fn message_override(key: &str, value: String) -> Option<String> {
    let allowed = OVERRIDABLE_MESSAGES
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, vars)| *vars)?;
    let trimmed = value.trim().to_string();
    if !(1..=4_000).contains(&utf16_len(&trimmed)) || trimmed.contains('<') {
        return None;
    }
    let vars: Vec<&str> = MESSAGE_VAR_RE
        .captures_iter(&trimmed)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    let same = vars.iter().all(|v| allowed.contains(v)) && allowed.iter().all(|v| vars.contains(v));
    same.then_some(trimmed)
}

impl SettingsDoc for I18nSettings {
    fn normalize(self) -> Option<Self> {
        let overrides = self
            .overrides
            .into_iter()
            .map(|(k, v)| message_override(&k, v).map(|v| (k, v)))
            .collect::<Option<_>>()?;
        Some(Self { overrides })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct SecuritySettings {
    #[serde(deserialize_with = "nullable")]
    pub contact: Option<String>,
}

impl SettingsDoc for SecuritySettings {
    fn normalize(self) -> Option<Self> {
        Some(Self {
            contact: opt(self.contact, security_contact)?,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct OperatorSettings {
    #[serde(deserialize_with = "nullable")]
    pub business_name: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub representative: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub registration_number: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub mail_order_number: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub address: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub phone: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub support_email: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub business_info_url: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub hosting_provider: Option<String>,
}

fn support_email(value: String) -> Option<String> {
    (utf16_len(&value) <= 320 && is_zod_email(&value)).then_some(value)
}

/// `https:` only (the value becomes an anchor href), and no control
/// characters, which the URL parser would silently strip from the stored text.
fn business_info_url(value: String) -> Option<String> {
    if utf16_len(&value) > 2048 || !no_control(&value) {
        return None;
    }
    let parsed = url::Url::parse(&value).ok()?;
    (parsed.scheme() == "https").then_some(value)
}

impl SettingsDoc for OperatorSettings {
    fn normalize(self) -> Option<Self> {
        Some(Self {
            business_name: opt(self.business_name, operator_text)?,
            representative: opt(self.representative, operator_text)?,
            registration_number: opt(self.registration_number, operator_text)?,
            mail_order_number: opt(self.mail_order_number, operator_text)?,
            address: opt(self.address, operator_text)?,
            phone: opt(self.phone, operator_text)?,
            support_email: opt(self.support_email, support_email)?,
            business_info_url: opt(self.business_info_url, business_info_url)?,
            hosting_provider: opt(self.hosting_provider, operator_text)?,
        })
    }
}

/// The effective value of every key (serialized as the admin `values`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct SettingsValues {
    pub branding: BrandingSettings,
    #[serde(rename = "defaults.user")]
    pub defaults_user: DefaultsUserSettings,
    pub auth: AuthSettings,
    pub share: SharePolicy,
    pub embed: EmbedSettings,
    pub features: FeaturesSettings,
    #[serde(rename = "attachmentPreview")]
    pub attachment_preview: AttachmentPreviewSettings,
    pub i18n: I18nSettings,
    pub security: SecuritySettings,
    pub operator: OperatorSettings,
}

impl SettingsValues {
    pub fn defaults(brand_name: &str) -> Self {
        Self {
            branding: BrandingSettings::default_named(brand_name),
            defaults_user: DefaultsUserSettings::default(),
            auth: AuthSettings::default(),
            share: SharePolicy::default(),
            embed: EmbedSettings::default(),
            features: FeaturesSettings::default(),
            attachment_preview: AttachmentPreviewSettings::default(),
            i18n: I18nSettings::default(),
            security: SecuritySettings::default(),
            operator: OperatorSettings::default(),
        }
    }

    pub fn get_json(&self, key: SettingsKey) -> Value {
        let value = match key {
            SettingsKey::Branding => serde_json::to_value(&self.branding),
            SettingsKey::DefaultsUser => serde_json::to_value(&self.defaults_user),
            SettingsKey::Auth => serde_json::to_value(&self.auth),
            SettingsKey::Share => serde_json::to_value(&self.share),
            SettingsKey::Embed => serde_json::to_value(&self.embed),
            SettingsKey::Features => serde_json::to_value(&self.features),
            SettingsKey::AttachmentPreview => serde_json::to_value(&self.attachment_preview),
            SettingsKey::I18n => serde_json::to_value(&self.i18n),
            SettingsKey::Security => serde_json::to_value(&self.security),
            SettingsKey::Operator => serde_json::to_value(&self.operator),
        };
        value.unwrap_or(Value::Null)
    }

    /// Parses and stores `value` into `key`; `None` when it fails the schema.
    pub fn set_json(&mut self, key: SettingsKey, value: &Value) -> Option<()> {
        match key {
            SettingsKey::Branding => self.branding = parse_doc(value)?,
            SettingsKey::DefaultsUser => self.defaults_user = parse_doc(value)?,
            SettingsKey::Auth => self.auth = parse_doc(value)?,
            SettingsKey::Share => self.share = parse_doc(value)?,
            SettingsKey::Embed => self.embed = parse_doc(value)?,
            SettingsKey::Features => self.features = parse_doc(value)?,
            SettingsKey::AttachmentPreview => self.attachment_preview = parse_doc(value)?,
            SettingsKey::I18n => self.i18n = parse_doc(value)?,
            SettingsKey::Security => self.security = parse_doc(value)?,
            SettingsKey::Operator => self.operator = parse_doc(value)?,
        }
        Some(())
    }

    pub fn to_json(&self, include: impl Fn(SettingsKey) -> bool) -> serde_json::Map<String, Value> {
        SETTINGS_KEYS
            .into_iter()
            .filter(|key| include(*key))
            .map(|key| (key.as_str().to_string(), self.get_json(key)))
            .collect()
    }
}

/// Validates one PATCH value (non-null) and returns its normalized JSON. The
/// `branding` input omits the asset leaves; the caller re-attaches the current
/// ones.
pub fn parse_patch_value(key: SettingsKey, value: &Value) -> Option<Value> {
    let mut probe = SettingsValues::defaults("FVOCI");
    if key == SettingsKey::Branding {
        let patch: BrandingPatch = serde_json::from_value(value.clone()).ok()?;
        let full = BrandingSettings {
            name: patch.name,
            smtp_from_display: patch.smtp_from_display,
            logo: None,
            favicon: None,
            login_brand_text: patch.login_brand_text,
        }
        .normalize()?;
        return serde_json::to_value(full).ok();
    }
    probe.set_json(key, value)?;
    Some(probe.get_json(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn branding_requires_every_nullable_leaf_and_rejects_control_chars() {
        assert!(parse_patch_value(SettingsKey::Branding, &json!({"name": "A"})).is_none());
        let ok = json!({"name": "  Acme  ", "smtpFromDisplay": null, "loginBrandText": null});
        assert_eq!(
            parse_patch_value(SettingsKey::Branding, &ok).unwrap()["name"],
            json!("Acme")
        );
        let bad = json!({"name": "A\r\nB", "smtpFromDisplay": null, "loginBrandText": null});
        assert!(parse_patch_value(SettingsKey::Branding, &bad).is_none());
        let asset = json!({"name": "A", "smtpFromDisplay": null, "loginBrandText": null,
            "logo": null});
        assert!(parse_patch_value(SettingsKey::Branding, &asset).is_none());
        let long =
            json!({"name": "가".repeat(81), "smtpFromDisplay": null, "loginBrandText": null});
        assert!(parse_patch_value(SettingsKey::Branding, &long).is_none());
    }

    #[test]
    fn share_refine_and_ranges() {
        let v =
            |d: i64, m: i64| json!({"enabled": true, "defaultExpiresDays": d, "maxExpiresDays": m});
        assert!(parse_patch_value(SettingsKey::Share, &v(7, 30)).is_some());
        assert!(parse_patch_value(SettingsKey::Share, &v(31, 30)).is_none());
        assert!(parse_patch_value(SettingsKey::Share, &v(0, 30)).is_none());
        assert!(parse_patch_value(SettingsKey::Share, &v(7, 366)).is_none());
    }

    #[test]
    fn message_overrides_keep_exact_variables() {
        let ok = json!({"overrides": {"mail.magic.link.text": " {{url}} in {{minutes}} "}});
        assert_eq!(
            parse_patch_value(SettingsKey::I18n, &ok).unwrap()["overrides"]["mail.magic.link.text"],
            json!("{{url}} in {{minutes}}")
        );
        for bad in [
            json!({"overrides": {"mail.magic.link.text": "{{url}} only"}}),
            json!({"overrides": {"mail.magic.link.text": "{{url}} {{minutes}} {{x}}"}}),
            json!({"overrides": {"constructor": "x"}}),
            json!({"overrides": {"mail.invite.subject": "<b>hi</b>"}}),
            json!({"overrides": {"mail.invite.subject": "   "}}),
        ] {
            assert!(
                parse_patch_value(SettingsKey::I18n, &bad).is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn security_contact_and_operator_urls() {
        let c = |v: &str| json!({"contact": v});
        assert!(parse_patch_value(SettingsKey::Security, &c("mailto:sec@example.com")).is_some());
        assert!(parse_patch_value(SettingsKey::Security, &c("https://example.com/sec")).is_some());
        assert!(parse_patch_value(SettingsKey::Security, &c("https://")).is_none());
        assert!(parse_patch_value(SettingsKey::Security, &c("mailto:nope")).is_none());
        assert!(parse_patch_value(SettingsKey::Security, &c("http://example.com")).is_none());
        let mut op = serde_json::to_value(OperatorSettings::default()).unwrap();
        op["businessInfoUrl"] = json!("javascript:alert(1)");
        assert!(parse_patch_value(SettingsKey::Operator, &op).is_none());
        op["businessInfoUrl"] = json!("https://biz.example.com/info");
        assert!(parse_patch_value(SettingsKey::Operator, &op).is_some());
        op["supportEmail"] = json!("..a@example.com");
        assert!(parse_patch_value(SettingsKey::Operator, &op).is_none());
    }

    #[test]
    fn defaults_user_embed_auth_bounds() {
        assert!(parse_patch_value(
            SettingsKey::DefaultsUser,
            &json!({"locale": "en", "timezone": "UTC", "weekStartsOn": 1, "textScale": 16})
        )
        .is_none());
        assert!(parse_patch_value(
            SettingsKey::DefaultsUser,
            &json!({"locale": "ko", "timezone": "UTC", "weekStartsOn": 0, "textScale": 18})
        )
        .is_some());
        assert!(parse_patch_value(SettingsKey::Embed, &json!({"hosts": ["Bad Host"]})).is_none());
        assert!(parse_patch_value(SettingsKey::Auth, &json!({"passwordMinLength": 9})).is_none());
        assert!(parse_patch_value(SettingsKey::Auth, &json!({"passwordMinLength": 12})).is_some());
    }
}
