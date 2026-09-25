//! Instance-level providers from `OIDC_<KEY>_*` (source `oidc-providers.ts`,
//! `packages/config` OIDC entries).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKey {
    Google,
    Microsoft,
    Kakao,
    Naver,
    Generic,
}

pub const PROVIDER_ORDER: [ProviderKey; 5] = [
    ProviderKey::Google,
    ProviderKey::Microsoft,
    ProviderKey::Kakao,
    ProviderKey::Naver,
    ProviderKey::Generic,
];

impl ProviderKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
            Self::Kakao => "kakao",
            Self::Naver => "naver",
            Self::Generic => "generic",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        PROVIDER_ORDER.into_iter().find(|k| k.as_str() == raw)
    }

    fn env_prefix(self) -> &'static str {
        match self {
            Self::Google => "OIDC_GOOGLE",
            Self::Microsoft => "OIDC_MICROSOFT",
            Self::Kakao => "OIDC_KAKAO",
            Self::Naver => "OIDC_NAVER",
            Self::Generic => "OIDC_GENERIC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Oidc,
    /// Naver: OAuth2 code flow + profile API, no id_token.
    OAuth2Naver,
}

pub const NAVER_ISSUER: &str = "https://nid.naver.com";

pub fn naver_profile_url(issuer: &str) -> String {
    if issuer == NAVER_ISSUER {
        "https://openapi.naver.com/v1/nid/me".to_string()
    } else {
        format!("{issuer}/v1/nid/me")
    }
}

#[derive(Clone)]
pub struct ResolvedProvider {
    pub key: ProviderKey,
    pub label: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub kind: ProviderKind,
    pub scope: &'static str,
    /// Microsoft multi-tenant issuers (`common`, `organizations`,
    /// `consumers`) publish `{tenantid}` in discovery; the id_token's `tid`
    /// fills it.
    pub microsoft_tenant: Option<String>,
}

impl fmt::Debug for ResolvedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedProvider")
            .field("key", &self.key)
            .field("label", &self.label)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, Debug, Default)]
pub struct OidcSettings {
    pub providers: Vec<ResolvedProvider>,
    /// `OIDC_ALLOW_INSECURE`: plain http is accepted for loopback issuers only
    /// (local development and tests). Everything else must be https to a
    /// public address.
    pub allow_insecure_loopback: bool,
    /// `PUBLIC_URL`; the redirect URI is `<origin>/api/v1/auth/oidc/<p>/callback`.
    pub public_origin: String,
}

/// Source `issuerSchema`: trailing slashes trimmed.
pub fn normalize_issuer(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn preset(
    key: ProviderKey,
    issuer: Option<String>,
    tenant: &str,
) -> Option<(String, ProviderKind, &'static str, &'static str)> {
    let (default_issuer, kind, scope, label) = match key {
        ProviderKey::Google => (
            Some("https://accounts.google.com".to_string()),
            ProviderKind::Oidc,
            "openid email profile",
            "Google",
        ),
        ProviderKey::Microsoft => (
            Some(format!("https://login.microsoftonline.com/{tenant}/v2.0")),
            ProviderKind::Oidc,
            "openid email profile",
            "Microsoft",
        ),
        ProviderKey::Kakao => (
            Some("https://kauth.kakao.com".to_string()),
            ProviderKind::Oidc,
            "openid account_email",
            "Kakao",
        ),
        ProviderKey::Naver => (
            Some(NAVER_ISSUER.to_string()),
            ProviderKind::OAuth2Naver,
            "",
            "Naver",
        ),
        ProviderKey::Generic => (None, ProviderKind::Oidc, "openid email profile", "SSO"),
    };
    let issuer = issuer.or(default_issuer)?;
    Some((issuer, kind, scope, label))
}

impl OidcSettings {
    pub fn from_env(public_origin: &str) -> Result<Self, String> {
        let tenant = env_nonempty("OIDC_MICROSOFT_TENANT").unwrap_or_else(|| "common".into());
        let mut providers = Vec::new();
        for key in PROVIDER_ORDER {
            let prefix = key.env_prefix();
            let (Some(client_id), Some(client_secret)) = (
                env_nonempty(&format!("{prefix}_CLIENT_ID")),
                env_nonempty(&format!("{prefix}_CLIENT_SECRET")),
            ) else {
                continue;
            };
            let issuer = env_nonempty(&format!("{prefix}_ISSUER")).map(|v| normalize_issuer(&v));
            let Some((issuer, kind, scope, preset_label)) = preset(key, issuer, &tenant) else {
                continue;
            };
            let label = if key == ProviderKey::Generic {
                env_nonempty("OIDC_GENERIC_LABEL").unwrap_or_else(|| preset_label.to_string())
            } else {
                preset_label.to_string()
            };
            let microsoft_tenant = (key == ProviderKey::Microsoft).then(|| tenant.clone());
            providers.push(ResolvedProvider {
                key,
                label,
                issuer,
                client_id,
                client_secret,
                kind,
                scope,
                microsoft_tenant,
            });
        }
        Ok(Self {
            providers,
            allow_insecure_loopback: env_nonempty("OIDC_ALLOW_INSECURE").is_some(),
            public_origin: public_origin.trim_end_matches('/').to_string(),
        })
    }

    pub fn find(&self, key: ProviderKey) -> Option<&ResolvedProvider> {
        self.providers.iter().find(|p| p.key == key)
    }

    pub fn redirect_uri(&self, key: ProviderKey) -> String {
        format!(
            "{}/api/v1/auth/oidc/{}/callback",
            self.public_origin,
            key.as_str()
        )
    }

    /// Builds one provider the way `from_env` would (tests and tooling).
    pub fn provider(
        key: ProviderKey,
        client_id: &str,
        client_secret: &str,
        issuer: Option<&str>,
    ) -> Option<ResolvedProvider> {
        let (issuer, kind, scope, label) = preset(key, issuer.map(normalize_issuer), "common")?;
        Some(ResolvedProvider {
            key,
            label: label.to_string(),
            issuer,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            kind,
            scope,
            microsoft_tenant: (key == ProviderKey::Microsoft).then(|| "common".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_keys_round_trip() {
        for key in PROVIDER_ORDER {
            assert_eq!(ProviderKey::parse(key.as_str()), Some(key));
        }
        assert_eq!(ProviderKey::parse("github"), None);
    }

    #[test]
    fn presets_and_generic_requires_issuer() {
        let google = OidcSettings::provider(ProviderKey::Google, "id", "s", None).unwrap();
        assert_eq!(google.issuer, "https://accounts.google.com");
        assert_eq!(google.scope, "openid email profile");
        let kakao = OidcSettings::provider(ProviderKey::Kakao, "id", "s", None).unwrap();
        assert_eq!(kakao.scope, "openid account_email");
        let ms = OidcSettings::provider(ProviderKey::Microsoft, "id", "s", None).unwrap();
        assert_eq!(ms.issuer, "https://login.microsoftonline.com/common/v2.0");
        assert!(OidcSettings::provider(ProviderKey::Generic, "id", "s", None).is_none());
        let generic =
            OidcSettings::provider(ProviderKey::Generic, "id", "s", Some("https://idp.test//"))
                .unwrap();
        assert_eq!(generic.issuer, "https://idp.test");
        assert!(!format!("{generic:?}").contains("\"s\""));
        assert_eq!(
            naver_profile_url(NAVER_ISSUER),
            "https://openapi.naver.com/v1/nid/me"
        );
        assert_eq!(
            naver_profile_url("http://127.0.0.1:9"),
            "http://127.0.0.1:9/v1/nid/me"
        );
    }
}
