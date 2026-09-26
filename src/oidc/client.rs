//! Provider metadata and the code exchange: discovery and JWKS with a
//! bounded process-local cache, the token request, id_token validation and
//! the Naver OAuth2 profile call (source `oidcExchange` / `naverExchange`
//! via openid-client).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use crate::oidc::fetch::{FetchError, FetchPolicy};
use crate::oidc::jwt::{self, Expected, IssuerRule, JwkSet, JwtError};
use crate::oidc::providers::{naver_profile_url, ProviderKey, ResolvedProvider};

const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_MAX_ENTRIES: usize = 64;
/// An unknown `kid` refetches the JWKS at most this often per URI.
const JWKS_FORCED_REFRESH_MIN: Duration = Duration::from_secs(30);
const MICROSOFT_TENANT_TEMPLATE: &str = "{tenantid}";

#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("fetch: {0}")]
    Fetch(#[from] FetchError),
    #[error("id_token: {0}")]
    Token(#[from] JwtError),
    #[error("discovery issuer mismatch")]
    IssuerMismatch,
    #[error("provider response: {0}")]
    Response(&'static str),
}

#[derive(Default)]
struct CacheInner {
    discovery: HashMap<String, (Instant, Arc<Discovery>)>,
    jwks: HashMap<String, (Instant, Arc<JwkSet>)>,
    forced: HashMap<String, Instant>,
}

/// Shared by every request of one server (source: instance-scoped cache).
#[derive(Default)]
pub struct OidcCache {
    inner: Mutex<CacheInner>,
}

impl std::fmt::Debug for OidcCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OidcCache")
    }
}

fn insert_bounded<V>(map: &mut HashMap<String, (Instant, V)>, key: String, value: V) {
    if !map.contains_key(&key) && map.len() >= CACHE_MAX_ENTRIES {
        let now = Instant::now();
        map.retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
        if map.len() >= CACHE_MAX_ENTRIES {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
    }
    map.insert(key, (Instant::now(), value));
}

/// Microsoft multi-tenant (`common`, `organizations`, `consumers`) discovery
/// publishes `{tenantid}`; the id_token's `tid` fills it.
fn tenant_template(provider: &ResolvedProvider) -> bool {
    provider.key == ProviderKey::Microsoft
        && provider
            .microsoft_tenant
            .as_deref()
            .is_some_and(|t| matches!(t, "common" | "organizations" | "consumers"))
}

impl OidcCache {
    pub async fn discovery(
        &self,
        policy: FetchPolicy,
        provider: &ResolvedProvider,
    ) -> Result<Arc<Discovery>, ExchangeError> {
        if let Some((at, doc)) = self
            .inner
            .lock()
            .expect("cache")
            .discovery
            .get(&provider.issuer)
        {
            if at.elapsed() < CACHE_TTL {
                return Ok(doc.clone());
            }
        }
        let url = format!("{}/.well-known/openid-configuration", provider.issuer);
        let doc: Discovery = policy.get_json(&url, None).await?;
        let issuer_ok = doc.issuer == provider.issuer
            || (tenant_template(provider) && doc.issuer.contains(MICROSOFT_TENANT_TEMPLATE));
        if !issuer_ok {
            return Err(ExchangeError::IssuerMismatch);
        }
        // Every endpoint obeys the same outbound rules; the authorization
        // endpoint is where users are sent, so it must be https too.
        for endpoint in [
            &doc.authorization_endpoint,
            &doc.token_endpoint,
            &doc.jwks_uri,
        ] {
            policy.check_url(endpoint)?;
        }
        let doc = Arc::new(doc);
        insert_bounded(
            &mut self.inner.lock().expect("cache").discovery,
            provider.issuer.clone(),
            doc.clone(),
        );
        Ok(doc)
    }

    async fn jwks(
        &self,
        policy: FetchPolicy,
        uri: &str,
        force: bool,
    ) -> Result<Option<Arc<JwkSet>>, ExchangeError> {
        {
            let mut inner = self.inner.lock().expect("cache");
            if force {
                let recently = inner
                    .forced
                    .get(uri)
                    .is_some_and(|at| at.elapsed() < JWKS_FORCED_REFRESH_MIN);
                if recently {
                    return Ok(None);
                }
                insert_bounded_instant(&mut inner.forced, uri.to_string());
            } else if let Some((at, set)) = inner.jwks.get(uri) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(Some(set.clone()));
                }
            }
        }
        let set: JwkSet = policy.get_json(uri, None).await?;
        let set = Arc::new(set);
        insert_bounded(
            &mut self.inner.lock().expect("cache").jwks,
            uri.to_string(),
            set.clone(),
        );
        Ok(Some(set))
    }

    /// Verifies the id_token signature, refreshing the key set once when the
    /// token names a key the cached set does not have (rotation).
    pub async fn verify_id_token(
        &self,
        policy: FetchPolicy,
        discovery: &Discovery,
        id_token: &str,
    ) -> Result<Value, ExchangeError> {
        let parsed = jwt::parse(id_token)?;
        let set = self
            .jwks(policy, &discovery.jwks_uri, false)
            .await?
            .ok_or(ExchangeError::Response("jwks"))?;
        match jwt::verify_signature(&set, &parsed) {
            Ok(()) => Ok(parsed.claims),
            Err(JwtError::UnknownKey) => {
                let Some(fresh) = self.jwks(policy, &discovery.jwks_uri, true).await? else {
                    return Err(JwtError::UnknownKey.into());
                };
                jwt::verify_signature(&fresh, &parsed)?;
                Ok(parsed.claims)
            }
            Err(err) => Err(err.into()),
        }
    }
}

fn insert_bounded_instant(map: &mut HashMap<String, Instant>, key: String) {
    if !map.contains_key(&key) && map.len() >= CACHE_MAX_ENTRIES {
        map.retain(|_, at| at.elapsed() < JWKS_FORCED_REFRESH_MIN);
        if map.len() >= CACHE_MAX_ENTRIES {
            map.clear();
        }
    }
    map.insert(key, Instant::now());
}

/// Normalized claims of the signed-in external account.
#[derive(Debug, Clone)]
pub struct SocialProfile {
    pub sub: String,
    /// The issuer the subject was verified against: the discovery issuer
    /// (which the id_token `iss` matched), or the configured base URL for an
    /// OAuth2 provider without discovery. `sub` is only unique within it.
    pub issuer: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub email_verified: bool,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

fn string_claim(claims: &Value, name: &str) -> Option<String> {
    claims
        .get(name)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Provider claims are untrusted: only a well-formed address, lowercased
/// (source `normalizeProviderEmail`).
/// Stored emails are printable ASCII (the users / identity_links checks).
pub fn normalize_provider_email(raw: Option<String>) -> Option<String> {
    raw.and_then(|email| crate::validate::normalize_email(&email).ok())
        .filter(|email| email.bytes().all(|b| (b'!'..=b'~').contains(&b)))
}

/// `client_secret_post`, as the source's openid-client uses by default for a
/// client with a secret, unless the provider lists `client_secret_basic` but
/// not `client_secret_post`.
fn uses_post_auth(discovery: &Discovery) -> bool {
    !discovery
        .token_endpoint_auth_methods_supported
        .as_ref()
        .is_some_and(|methods| {
            methods.iter().any(|m| m == "client_secret_basic")
                && !methods.iter().any(|m| m == "client_secret_post")
        })
}

pub struct CodeExchange<'a> {
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub pkce_verifier: &'a str,
    pub nonce: &'a str,
    /// `iss` authorization response parameter (RFC 9207), when present.
    pub iss_param: Option<&'a str>,
    pub now_secs: i64,
}

/// Source `oidcExchange`: authorization code grant with PKCE, then the
/// id_token checks. The access token is never used or logged.
pub async fn oidc_exchange(
    cache: &OidcCache,
    policy: FetchPolicy,
    provider: &ResolvedProvider,
    input: CodeExchange<'_>,
) -> Result<SocialProfile, ExchangeError> {
    let discovery = cache.discovery(policy, provider).await?;
    if let Some(iss) = input.iss_param {
        if !tenant_template(provider) && iss != discovery.issuer {
            return Err(ExchangeError::IssuerMismatch);
        }
    } else if discovery.authorization_response_iss_parameter_supported == Some(true) {
        return Err(ExchangeError::Response("iss parameter missing"));
    }
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", input.code),
        ("redirect_uri", input.redirect_uri),
        ("code_verifier", input.pkce_verifier),
    ];
    let basic = if uses_post_auth(&discovery) {
        form.push(("client_id", &provider.client_id));
        form.push(("client_secret", &provider.client_secret));
        None
    } else {
        Some((provider.client_id.as_str(), provider.client_secret.as_str()))
    };
    let tokens: TokenResponse = policy
        .post_form(&discovery.token_endpoint, &form, basic)
        .await?;
    let id_token = tokens
        .id_token
        .ok_or(ExchangeError::Response("id_token missing"))?;
    let claims = cache.verify_id_token(policy, &discovery, &id_token).await?;
    let issuer = if tenant_template(provider) {
        IssuerRule::TenantTemplate(&discovery.issuer)
    } else {
        IssuerRule::Exact(&discovery.issuer)
    };
    jwt::check_claims(
        &claims,
        &Expected {
            issuer,
            client_id: &provider.client_id,
            nonce: input.nonce,
            now_secs: input.now_secs,
        },
    )?;
    Ok(SocialProfile {
        sub: string_claim(&claims, "sub").ok_or(ExchangeError::Response("sub"))?,
        issuer: discovery.issuer.clone(),
        email: normalize_provider_email(string_claim(&claims, "email")),
        name: string_claim(&claims, "name").or_else(|| string_claim(&claims, "nickname")),
        email_verified: claims.get("email_verified") == Some(&Value::Bool(true)),
    })
}

#[derive(Deserialize)]
struct NaverProfile {
    #[serde(default)]
    resultcode: Option<String>,
    #[serde(default)]
    response: Option<NaverProfileBody>,
}

#[derive(Deserialize)]
struct NaverProfileBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    nickname: Option<String>,
}

/// Source `naverExchange`: OAuth2 code grant and the profile API. Naver's
/// email is never treated as verified.
pub async fn naver_exchange(
    policy: FetchPolicy,
    provider: &ResolvedProvider,
    code: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<SocialProfile, ExchangeError> {
    let token: TokenResponse = policy
        .post_form(
            &format!("{}/oauth2.0/token", provider.issuer),
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &provider.client_id),
                ("client_secret", &provider.client_secret),
                ("redirect_uri", redirect_uri),
                ("code", code),
                ("state", state),
            ],
            None,
        )
        .await?;
    let access = token
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or(ExchangeError::Response("naver token"))?;
    let profile: NaverProfile = policy
        .get_json(&naver_profile_url(&provider.issuer), Some(&access))
        .await?;
    let body = profile
        .response
        .filter(|_| profile.resultcode.as_deref() == Some("00"))
        .ok_or(ExchangeError::Response("naver profile"))?;
    let sub = body
        .id
        .filter(|id| !id.is_empty())
        .ok_or(ExchangeError::Response("naver id"))?;
    Ok(SocialProfile {
        sub,
        issuer: provider.issuer.clone(),
        email: normalize_provider_email(body.email),
        name: body.name.or(body.nickname),
        email_verified: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discovery(methods: Option<Vec<&str>>) -> Discovery {
        Discovery {
            issuer: "https://idp.test".into(),
            authorization_endpoint: "https://idp.test/a".into(),
            token_endpoint: "https://idp.test/t".into(),
            jwks_uri: "https://idp.test/j".into(),
            token_endpoint_auth_methods_supported: methods
                .map(|m| m.into_iter().map(str::to_string).collect()),
            authorization_response_iss_parameter_supported: None,
        }
    }

    #[test]
    fn client_auth_method_selection() {
        assert!(uses_post_auth(&discovery(None)));
        assert!(uses_post_auth(&discovery(Some(vec![
            "client_secret_basic",
            "client_secret_post"
        ]))));
        assert!(uses_post_auth(&discovery(Some(vec!["client_secret_post"]))));
        assert!(!uses_post_auth(&discovery(Some(vec![
            "client_secret_basic"
        ]))));
    }

    #[test]
    fn provider_email_is_canonicalized_or_dropped() {
        assert_eq!(
            normalize_provider_email(Some(" Kim@Example.COM ".into())),
            Some("kim@example.com".into())
        );
        assert_eq!(normalize_provider_email(Some("not an email".into())), None);
        assert_eq!(
            normalize_provider_email(Some("김@example.com".into())),
            None
        );
        assert_eq!(normalize_provider_email(None), None);
    }

    #[test]
    fn cache_is_bounded() {
        let mut map: HashMap<String, (Instant, u8)> = HashMap::new();
        for i in 0..(CACHE_MAX_ENTRIES + 10) {
            insert_bounded(&mut map, format!("k{i}"), 0);
        }
        assert!(map.len() <= CACHE_MAX_ENTRIES);
    }
}
