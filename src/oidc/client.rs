//! Provider metadata and the code exchange (source `oidcExchange` /
//! `naverExchange` via openid-client).
//!
//! The `openidconnect` crate parses discovery, JWKS and id_tokens, shapes the
//! authorization URL and the token request, and verifies the id_token
//! signature and core claims. FVOCI keeps: the transport (every request goes
//! through [`FetchPolicy::send`]), the bounded discovery/JWKS cache with one
//! forced refresh per rotation window, the discovery issuer rule (including
//! Microsoft's `{tenantid}` template), the verifier configuration below, the
//! claim rules the crate leaves to the relying party, and the Naver OAuth2
//! profile call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, TimeDelta, Utc};
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreClaimName, CoreClaimType, CoreClientAuthMethod,
    CoreErrorResponseType, CoreGenderClaim, CoreGrantType, CoreIdTokenVerifier, CoreJsonWebKey,
    CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm,
    CoreJwsSigningAlgorithm, CoreResponseMode, CoreResponseType, CoreRevocableToken,
    CoreRevocationErrorResponse, CoreSubjectIdentifierType, CoreTokenIntrospectionResponse,
    CoreTokenType,
};
use openidconnect::{
    AdditionalClaims, AdditionalProviderMetadata, AuthType, AuthenticationFlow, AuthorizationCode,
    ClaimsVerificationError, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    EndpointNotSet, EndpointSet, IdTokenClaims, IdTokenFields, JsonWebKeySet, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, ProviderMetadata, RedirectUrl, RequestTokenError, Scope,
    SignatureVerificationError, StandardErrorResponse, StandardTokenResponse, TokenResponse as _,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::oidc::fetch::{FetchError, FetchPolicy};
use crate::oidc::providers::{naver_profile_url, ProviderKey, ResolvedProvider};

const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_MAX_ENTRIES: usize = 64;
/// An unknown `kid` refetches the JWKS at most this often per URI.
const JWKS_FORCED_REFRESH_MIN: Duration = Duration::from_secs(30);
const MICROSOFT_TENANT_TEMPLATE: &str = "{tenantid}";
/// Clock skew tolerated for `exp` and `nbf`.
pub const CLOCK_SKEW_SECS: i64 = 60;
/// An id_token issued further in the future than this is refused.
const MAX_FUTURE_IAT_SECS: i64 = 300;
const MAX_TOKEN_BYTES: usize = 16 * 1024;
/// RSA keys shorter than this are dropped from a JWKS (the `rsa` backend has
/// no lower bound of its own).
const MIN_RSA_MODULUS_BYTES: usize = 256;
/// The only id_token algorithms accepted (the crate default is RS256 only;
/// `none` and HMAC are never accepted).
const ALLOWED_ALGS: [CoreJwsSigningAlgorithm; 2] = [
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
    CoreJwsSigningAlgorithm::EcdsaP256Sha256,
];

/// RFC 9207 discovery flag, beyond the crate's core metadata.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IssParameter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authorization_response_iss_parameter_supported: Option<bool>,
}
impl AdditionalProviderMetadata for IssParameter {}

pub type Discovery = ProviderMetadata<
    IssParameter,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// id_token claims the crate does not model but FVOCI checks.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtraClaims {
    /// Microsoft tenant id, fills the `{tenantid}` issuer template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nbf: Option<f64>,
}
impl AdditionalClaims for ExtraClaims {}

type Claims = IdTokenClaims<ExtraClaims, CoreGenderClaim>;
type IdToken = openidconnect::IdToken<
    ExtraClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;
type TokenResponse = StandardTokenResponse<
    IdTokenFields<
        ExtraClaims,
        EmptyExtraTokenFields,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >,
    CoreTokenType,
>;
type OidcClient<Auth = EndpointSet, Token = EndpointSet> = openidconnect::Client<
    ExtraClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    TokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    Auth,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    Token,
    EndpointNotSet,
>;

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("fetch: {0}")]
    Fetch(#[from] FetchError),
    #[error("id_token: {0}")]
    Token(&'static str),
    #[error("discovery issuer mismatch")]
    IssuerMismatch,
    #[error("provider response: {0}")]
    Response(&'static str),
}

/// A fixed reason per failure: no token, claim or provider text is logged.
fn claims_error(err: &ClaimsVerificationError) -> ExchangeError {
    ExchangeError::Token(match err {
        ClaimsVerificationError::Expired(_) => "exp/iat",
        ClaimsVerificationError::InvalidAudience(_) => "aud",
        ClaimsVerificationError::InvalidIssuer(_) => "iss",
        ClaimsVerificationError::InvalidNonce(_) => "nonce",
        ClaimsVerificationError::SignatureVerification(sig) => match sig {
            SignatureVerificationError::NoMatchingKey => "no matching key",
            SignatureVerificationError::AmbiguousKeyId(_) => "ambiguous key",
            SignatureVerificationError::DisallowedAlg(_)
            | SignatureVerificationError::NoSignature
            | SignatureVerificationError::UnsupportedAlg(_) => "algorithm",
            _ => "signature",
        },
        _ => "unsupported",
    })
}

#[derive(Default)]
struct CacheInner {
    discovery: HashMap<String, (Instant, Arc<Discovery>)>,
    jwks: HashMap<String, (Instant, Arc<CoreJsonWebKeySet>)>,
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

fn insert_bounded_instant(map: &mut HashMap<String, Instant>, key: String) {
    if !map.contains_key(&key) && map.len() >= CACHE_MAX_ENTRIES {
        map.retain(|_, at| at.elapsed() < JWKS_FORCED_REFRESH_MIN);
        if map.len() >= CACHE_MAX_ENTRIES {
            map.clear();
        }
    }
    map.insert(key, Instant::now());
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

/// Drops RSA keys under 2048 bits before the crate sees the set.
fn without_short_rsa_keys(mut raw: Value) -> Value {
    if let Some(keys) = raw.get_mut("keys").and_then(Value::as_array_mut) {
        keys.retain(|key| {
            key.get("kty").and_then(Value::as_str) != Some("RSA")
                || key
                    .get("n")
                    .and_then(Value::as_str)
                    .and_then(|n| URL_SAFE_NO_PAD.decode(n).ok())
                    .is_some_and(|n| {
                        n.iter().skip_while(|b| **b == 0).count() >= MIN_RSA_MODULUS_BYTES
                    })
        });
    }
    raw
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
        let issuer = doc.issuer().as_str();
        let issuer_ok = issuer == provider.issuer
            || (tenant_template(provider) && issuer.contains(MICROSOFT_TENANT_TEMPLATE));
        if !issuer_ok {
            return Err(ExchangeError::IssuerMismatch);
        }
        let token_endpoint = doc
            .token_endpoint()
            .ok_or(ExchangeError::Response("token endpoint missing"))?;
        // Every endpoint obeys the same outbound rules; the authorization
        // endpoint is where users are sent, so it must be https too.
        for endpoint in [
            doc.authorization_endpoint().as_str(),
            token_endpoint.as_str(),
            doc.jwks_uri().as_str(),
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
    ) -> Result<Option<Arc<CoreJsonWebKeySet>>, ExchangeError> {
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
        let raw: Value = policy.get_json(uri, None).await?;
        let set: CoreJsonWebKeySet = serde_json::from_value(without_short_rsa_keys(raw))
            .map_err(|_| ExchangeError::Response("jwks"))?;
        let set = Arc::new(set);
        insert_bounded(
            &mut self.inner.lock().expect("cache").jwks,
            uri.to_string(),
            set.clone(),
        );
        Ok(Some(set))
    }

    /// Verifies the id_token, refreshing the key set once when the token
    /// names a key the cached set does not have (rotation).
    async fn verify_id_token(
        &self,
        policy: FetchPolicy,
        discovery: &Discovery,
        id_token: &IdToken,
        check: &IdTokenCheck<'_>,
    ) -> Result<Claims, ExchangeError> {
        let set = self
            .jwks(policy, discovery.jwks_uri().as_str(), false)
            .await?
            .ok_or(ExchangeError::Response("jwks"))?;
        match verify_with(&set, id_token, check) {
            Err(ExchangeError::Token("no matching key")) => {
                let Some(fresh) = self
                    .jwks(policy, discovery.jwks_uri().as_str(), true)
                    .await?
                else {
                    return Err(ExchangeError::Token("no matching key"));
                };
                verify_with(&fresh, id_token, check)
            }
            other => other,
        }
    }
}

/// What the id_token must satisfy for this exchange.
struct IdTokenCheck<'a> {
    discovery: &'a Discovery,
    tenant_template: bool,
    client_id: &'a str,
    nonce: &'a str,
    now: DateTime<Utc>,
}

/// The crate's verifier, configured where its defaults differ from FVOCI's
/// policy: RS256/ES256 only, `exp` with [`CLOCK_SKEW_SECS`], `iat` at most
/// [`MAX_FUTURE_IAT_SECS`] ahead, no HMAC (public-client verifier, so the
/// client secret is never a key), and for the Microsoft template the issuer
/// is checked below instead.
fn verifier<'a>(set: &CoreJsonWebKeySet, check: &IdTokenCheck<'a>) -> CoreIdTokenVerifier<'a> {
    let now = check.now;
    CoreIdTokenVerifier::new_public_client(
        ClientId::new(check.client_id.to_string()),
        check.discovery.issuer().clone(),
        set.clone(),
    )
    .set_allowed_algs(ALLOWED_ALGS)
    .require_issuer_match(!check.tenant_template)
    .set_time_fn(move || now - TimeDelta::seconds(CLOCK_SKEW_SECS))
    .set_issue_time_verifier_fn(move |iat| {
        if iat > now + TimeDelta::seconds(MAX_FUTURE_IAT_SECS) {
            Err("issued in the future".into())
        } else {
            Ok(())
        }
    })
}

fn verify_with(
    set: &CoreJsonWebKeySet,
    id_token: &IdToken,
    check: &IdTokenCheck<'_>,
) -> Result<Claims, ExchangeError> {
    let claims = id_token
        .claims(&verifier(set, check), &Nonce::new(check.nonce.to_string()))
        .map_err(|err| claims_error(&err))?;
    relying_party_rules(claims, check)?;
    Ok(claims.clone())
}

/// Checks the crate leaves to the relying party.
fn relying_party_rules(claims: &Claims, check: &IdTokenCheck<'_>) -> Result<(), ExchangeError> {
    if check.tenant_template {
        let tid = claims
            .additional_claims()
            .tid
            .as_deref()
            .filter(|tid| {
                !tid.is_empty() && tid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            .ok_or(ExchangeError::Token("iss"))?;
        let expected = check
            .discovery
            .issuer()
            .as_str()
            .replace(MICROSOFT_TENANT_TEMPLATE, tid);
        if claims.issuer().as_str() != expected {
            return Err(ExchangeError::Token("iss"));
        }
    }
    // Other audiences are already refused by the crate; a present `azp`
    // must still name this client.
    if claims
        .authorized_party()
        .is_some_and(|azp| azp.as_str() != check.client_id)
    {
        return Err(ExchangeError::Token("azp"));
    }
    if let Some(nbf) = claims.additional_claims().nbf {
        let limit = (check.now.timestamp() + CLOCK_SKEW_SECS) as f64;
        if !nbf.is_finite() || nbf > limit {
            return Err(ExchangeError::Token("nbf"));
        }
    }
    let sub = claims.subject().as_str();
    if sub.is_empty() || sub.len() > 255 {
        return Err(ExchangeError::Token("sub"));
    }
    Ok(())
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
        .token_endpoint_auth_methods_supported()
        .is_some_and(|methods| {
            methods.contains(&CoreClientAuthMethod::ClientSecretBasic)
                && !methods.contains(&CoreClientAuthMethod::ClientSecretPost)
        })
}

fn oidc_client(
    discovery: &Discovery,
    provider: &ResolvedProvider,
    redirect_uri: &str,
) -> Result<OidcClient, ExchangeError> {
    let token_endpoint = discovery
        .token_endpoint()
        .cloned()
        .ok_or(ExchangeError::Response("token endpoint missing"))?;
    let redirect_uri = RedirectUrl::new(redirect_uri.to_string())
        .map_err(|_| ExchangeError::Response("redirect uri"))?;
    Ok(OidcClient::<EndpointNotSet, EndpointNotSet>::new(
        ClientId::new(provider.client_id.clone()),
        discovery.issuer().clone(),
        JsonWebKeySet::default(),
    )
    .set_client_secret(ClientSecret::new(provider.client_secret.clone()))
    .set_auth_uri(discovery.authorization_endpoint().clone())
    .set_token_uri(token_endpoint)
    .set_redirect_uri(redirect_uri)
    .set_auth_type(if uses_post_auth(discovery) {
        AuthType::RequestBody
    } else {
        AuthType::BasicAuth
    }))
}

/// The authorization request: code flow, the provider's scopes, `state`,
/// `nonce` and a PKCE S256 challenge of `pkce_verifier`.
pub fn authorization_url(
    discovery: &Discovery,
    provider: &ResolvedProvider,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    pkce_verifier: &str,
) -> Result<String, ExchangeError> {
    let client = oidc_client(discovery, provider, redirect_uri)?;
    let challenge =
        PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(pkce_verifier.into()));
    let (state, nonce) = (state.to_string(), nonce.to_string());
    let mut request = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            move || CsrfToken::new(state),
            move || Nonce::new(nonce),
        )
        .set_pkce_challenge(challenge);
    // The crate always requests `openid`.
    for scope in provider
        .scope
        .split(' ')
        .filter(|s| !s.is_empty() && *s != "openid")
    {
        request = request.add_scope(Scope::new(scope.to_string()));
    }
    Ok(request.url().0.to_string())
}

pub struct CodeExchange<'a> {
    pub code: &'a str,
    pub redirect_uri: &'a str,
    pub pkce_verifier: &'a str,
    pub nonce: &'a str,
    /// `iss` authorization response parameter (RFC 9207), when present.
    pub iss_param: Option<&'a str>,
    pub now: DateTime<Utc>,
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
    let template = tenant_template(provider);
    if let Some(iss) = input.iss_param {
        if !template && iss != discovery.issuer().as_str() {
            return Err(ExchangeError::IssuerMismatch);
        }
    } else if discovery
        .additional_metadata()
        .authorization_response_iss_parameter_supported
        == Some(true)
    {
        return Err(ExchangeError::Response("iss parameter missing"));
    }
    let http = move |request| async move { policy.send(request).await };
    let tokens = oidc_client(&discovery, provider, input.redirect_uri)?
        .exchange_code(AuthorizationCode::new(input.code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(input.pkce_verifier.to_string()))
        .request_async(&http)
        .await
        .map_err(|err| match err {
            RequestTokenError::Request(fetch) => ExchangeError::Fetch(fetch),
            RequestTokenError::ServerResponse(_) => ExchangeError::Response("token error"),
            _ => ExchangeError::Response("token response"),
        })?;
    let id_token = tokens
        .id_token()
        .ok_or(ExchangeError::Response("id_token missing"))?;
    if id_token.to_string().len() > MAX_TOKEN_BYTES {
        return Err(ExchangeError::Token("too large"));
    }
    let claims = cache
        .verify_id_token(
            policy,
            &discovery,
            id_token,
            &IdTokenCheck {
                discovery: &discovery,
                tenant_template: template,
                client_id: &provider.client_id,
                nonce: input.nonce,
                now: input.now,
            },
        )
        .await?;
    let text = |value: Option<&str>| value.filter(|v| !v.is_empty()).map(str::to_string);
    Ok(SocialProfile {
        sub: claims.subject().as_str().to_string(),
        issuer: discovery.issuer().as_str().to_string(),
        email: normalize_provider_email(text(claims.email().map(|e| e.as_str()))),
        name: text(claims.name().and_then(|n| n.get(None)).map(|n| n.as_str())).or_else(|| {
            text(
                claims
                    .nickname()
                    .and_then(|n| n.get(None))
                    .map(|n| n.as_str()),
            )
        }),
        email_verified: claims.email_verified() == Some(true),
    })
}

/// Naver answers errors with 200 and sends `expires_in` as a string, so its
/// token response is read directly rather than as an RFC 6749 response.
#[derive(Deserialize)]
struct NaverToken {
    #[serde(default)]
    access_token: Option<String>,
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
    let token: NaverToken = policy
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
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
    use serde_json::json;
    use std::str::FromStr;

    const NOW: i64 = 2_000_000_050;

    struct Signer {
        pair: EcdsaKeyPair,
        kid: String,
    }

    impl Signer {
        fn new(kid: &str) -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
            let pair =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
                    .unwrap();
            Self {
                pair,
                kid: kid.into(),
            }
        }

        fn jwk(&self) -> Value {
            let point = self.pair.public_key().as_ref();
            json!({
                "kty": "EC", "kid": self.kid, "use": "sig", "alg": "ES256", "crv": "P-256",
                "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
                "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
            })
        }

        fn sign_with(&self, header: Value, claims: &Value) -> String {
            let input = signing_input(&header, claims);
            let sig = self
                .pair
                .sign(&SystemRandom::new(), input.as_bytes())
                .unwrap();
            format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref()))
        }

        fn sign(&self, claims: &Value) -> String {
            self.sign_with(json!({"alg": "ES256", "kid": self.kid}), claims)
        }
    }

    fn signing_input(header: &Value, claims: &Value) -> String {
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    fn set(keys: Vec<Value>) -> CoreJsonWebKeySet {
        serde_json::from_value(json!({ "keys": keys })).unwrap()
    }

    fn discovery(extra: Value) -> Discovery {
        let mut doc = json!({
            "issuer": "https://idp.test",
            "authorization_endpoint": "https://idp.test/a",
            "token_endpoint": "https://idp.test/t",
            "jwks_uri": "https://idp.test/j",
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256", "ES256"],
        });
        for (k, v) in extra.as_object().unwrap() {
            doc[k] = v.clone();
        }
        serde_json::from_value(doc).unwrap()
    }

    fn claims() -> Value {
        json!({
            "iss": "https://idp.test", "aud": "client", "sub": "user-1",
            "exp": NOW + 50, "iat": NOW - 50, "nonce": "n-1",
        })
    }

    fn verify(
        doc: &Discovery,
        template: bool,
        keys: &CoreJsonWebKeySet,
        token: &str,
    ) -> Result<Claims, ExchangeError> {
        let id_token = IdToken::from_str(token).map_err(|_| ExchangeError::Response("parse"))?;
        verify_with(
            keys,
            &id_token,
            &IdTokenCheck {
                discovery: doc,
                tenant_template: template,
                client_id: "client",
                nonce: "n-1",
                now: DateTime::from_timestamp(NOW, 0).unwrap(),
            },
        )
    }

    fn reason(result: Result<Claims, ExchangeError>) -> &'static str {
        match result {
            Err(ExchangeError::Token(reason) | ExchangeError::Response(reason)) => reason,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn es256_is_allowed_and_tampering_is_refused() {
        // The crate's default allow-list is RS256 only.
        let signer = Signer::new("k1");
        let keys = set(vec![signer.jwk()]);
        let doc = discovery(json!({}));
        let token = signer.sign(&claims());
        let verified = verify(&doc, false, &keys, &token).unwrap();
        assert_eq!(verified.subject().as_str(), "user-1");

        let mut forged = claims();
        forged["sub"] = json!("admin");
        let other = signer.sign(&forged);
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = other.split('.').nth(1).unwrap();
        assert_eq!(
            reason(verify(&doc, false, &keys, &parts.join("."))),
            "signature"
        );

        // Another key under the same kid.
        let impostor = Signer::new("k1");
        assert_eq!(
            reason(verify(&doc, false, &keys, &impostor.sign(&claims()))),
            "signature"
        );
    }

    #[test]
    fn none_hmac_and_other_algorithms_are_refused() {
        let signer = Signer::new("k1");
        let jwks = json!({ "keys": [signer.jwk()] });
        let keys = set(vec![signer.jwk()]);
        let doc = discovery(json!({}));
        let body = claims();

        let none = format!("{}.", signing_input(&json!({"alg": "none"}), &body));
        assert!(verify(&doc, false, &keys, &none).is_err());

        // HS256 keyed with the published JWKS or with the client secret.
        for secret in [jwks.to_string(), "client-secret".to_string()] {
            let input = signing_input(&json!({"alg": "HS256", "kid": "k1"}), &body);
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
            let tag = ring::hmac::sign(&key, input.as_bytes());
            let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(tag.as_ref()));
            assert_eq!(reason(verify(&doc, false, &keys, &token)), "algorithm");
        }
        for alg in ["RS512", "PS256", "ES384", "EdDSA"] {
            let token = signer.sign_with(json!({"alg": alg, "kid": "k1"}), &body);
            assert_eq!(
                reason(verify(&doc, false, &keys, &token)),
                "algorithm",
                "{alg}"
            );
        }
        let crit = signer.sign_with(json!({"alg": "ES256", "kid": "k1", "crit": ["x"]}), &body);
        assert!(verify(&doc, false, &keys, &crit).is_err());
    }

    #[test]
    fn key_selection_by_kid_alg_and_use() {
        let signer = Signer::new("k1");
        let doc = discovery(json!({}));
        let token = signer.sign(&claims());
        // Unknown kid: the caller refreshes the set once.
        let other = Signer::new("k2");
        assert_eq!(
            reason(verify(&doc, false, &set(vec![other.jwk()]), &token)),
            "no matching key"
        );
        // An encryption key or a key for another algorithm is not a candidate.
        let mut enc = signer.jwk();
        enc["use"] = json!("enc");
        assert_eq!(
            reason(verify(&doc, false, &set(vec![enc]), &token)),
            "no matching key"
        );
        let mut rs = signer.jwk();
        rs["alg"] = json!("RS256");
        assert_eq!(
            reason(verify(&doc, false, &set(vec![rs]), &token)),
            "no matching key"
        );
        let mut p384 = signer.jwk();
        p384["crv"] = json!("P-384");
        assert!(verify(&doc, false, &set(vec![p384]), &token).is_err());
    }

    #[test]
    fn time_claims_use_fvoci_skew() {
        let signer = Signer::new("k1");
        let keys = set(vec![signer.jwk()]);
        let doc = discovery(json!({}));
        let with = |name: &str, value: Value| {
            let mut c = claims();
            c[name] = value;
            verify(&doc, false, &keys, &signer.sign(&c))
        };
        // Crate default: no skew. FVOCI: exp may be up to CLOCK_SKEW_SECS past.
        assert!(with("exp", json!(NOW - CLOCK_SKEW_SECS + 1)).is_ok());
        assert_eq!(reason(with("exp", json!(NOW - CLOCK_SKEW_SECS))), "exp/iat");
        // Crate default: any iat. FVOCI: at most MAX_FUTURE_IAT_SECS ahead.
        assert!(with("iat", json!(NOW + MAX_FUTURE_IAT_SECS)).is_ok());
        assert_eq!(
            reason(with("iat", json!(NOW + MAX_FUTURE_IAT_SECS + 1))),
            "exp/iat"
        );
        // nbf (not checked by the crate) with the same skew.
        assert!(with("nbf", json!(NOW + CLOCK_SKEW_SECS)).is_ok());
        assert_eq!(reason(with("nbf", json!(NOW + CLOCK_SKEW_SECS + 1))), "nbf");
    }

    #[test]
    fn audience_nonce_issuer_and_subject_rules() {
        let signer = Signer::new("k1");
        let keys = set(vec![signer.jwk()]);
        let doc = discovery(json!({}));
        let with = |edit: &dyn Fn(&mut Value)| {
            let mut c = claims();
            edit(&mut c);
            verify(&doc, false, &keys, &signer.sign(&c))
        };
        assert_eq!(
            reason(with(&|c| c["iss"] = json!("https://evil.test"))),
            "iss"
        );
        assert_eq!(
            reason(with(&|c| c["iss"] = json!("https://idp.test/"))),
            "iss"
        );
        assert_eq!(reason(with(&|c| c["aud"] = json!("other"))), "aud");
        // An untrusted extra audience is refused even with a matching azp.
        assert_eq!(
            reason(with(&|c| {
                c["aud"] = json!(["client", "other"]);
                c["azp"] = json!("client");
            })),
            "aud"
        );
        assert!(with(&|c| c["azp"] = json!("client")).is_ok());
        assert_eq!(reason(with(&|c| c["azp"] = json!("other"))), "azp");
        assert_eq!(reason(with(&|c| c["nonce"] = json!("n-2"))), "nonce");
        assert_eq!(
            reason(with(&|c| {
                c.as_object_mut().unwrap().remove("nonce");
            })),
            "nonce"
        );
        assert!(with(&|c| c["sub"] = json!("")).is_err());
        assert_eq!(reason(with(&|c| c["sub"] = json!("s".repeat(256)))), "sub");

        let ms = discovery(json!({"issuer": "https://login.microsoftonline.com/{tenantid}/v2.0"}));
        let tenant = |iss: &str, tid: Value| {
            let mut c = claims();
            c["iss"] = json!(iss);
            c["tid"] = tid;
            verify(&ms, true, &keys, &signer.sign(&c))
        };
        let iss = "https://login.microsoftonline.com/abc-123/v2.0";
        assert!(tenant(iss, json!("abc-123")).is_ok());
        assert_eq!(reason(tenant(iss, json!("other"))), "iss");
        assert_eq!(reason(tenant(iss, json!("abc-123/../x"))), "iss");
        assert_eq!(reason(tenant(iss, Value::Null)), "iss");
        // Without the template rule the literal template issuer never matches.
        assert_eq!(
            reason(verify(
                &ms,
                false,
                &keys,
                &signer.sign(&{
                    let mut c = claims();
                    c["iss"] = json!(iss);
                    c["tid"] = json!("abc-123");
                    c
                })
            )),
            "iss"
        );
    }

    #[test]
    fn email_verified_must_be_a_boolean() {
        // No `accept-string-booleans`: a string is refused, not coerced.
        let signer = Signer::new("k1");
        let mut c = claims();
        c["email_verified"] = json!("true");
        assert!(IdToken::from_str(&signer.sign(&c)).is_err());
        c["email_verified"] = json!(true);
        let keys = set(vec![signer.jwk()]);
        let verified = verify(&discovery(json!({})), false, &keys, &signer.sign(&c)).unwrap();
        assert_eq!(verified.email_verified(), Some(true));
    }

    #[test]
    fn short_rsa_keys_are_dropped() {
        let rsa = |bytes: usize| {
            let mut n = vec![0xc5u8; bytes];
            n.insert(0, 0);
            json!({"kty": "RSA", "kid": format!("r{bytes}"), "n": URL_SAFE_NO_PAD.encode(n), "e": "AQAB"})
        };
        let ec = Signer::new("e").jwk();
        let raw = json!({"keys": [rsa(128), rsa(255), rsa(256), rsa(512), ec]});
        let kept = without_short_rsa_keys(raw);
        let kids: Vec<&str> = kept["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["kid"].as_str().unwrap())
            .collect();
        assert_eq!(kids, ["r256", "r512", "e"]);
    }

    #[test]
    fn authorization_url_carries_pkce_nonce_state_and_scopes() {
        let provider =
            crate::oidc::OidcSettings::provider(ProviderKey::Kakao, "client", "secret", None)
                .unwrap();
        let url = authorization_url(
            &discovery(json!({})),
            &provider,
            "https://app.test/cb",
            "st",
            "no",
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        )
        .unwrap();
        let url = url::Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/a");
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let expected = [
            ("response_type", "code"),
            ("client_id", "client"),
            ("redirect_uri", "https://app.test/cb"),
            ("state", "st"),
            ("nonce", "no"),
            ("scope", "openid account_email"),
            // RFC 7636 appendix B.
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ];
        for (name, value) in expected {
            assert_eq!(params.get(name).map(String::as_str), Some(value), "{name}");
        }
        // Nothing else (no response_mode, prompt or client secret).
        assert_eq!(params.len(), expected.len(), "{params:?}");
    }

    #[test]
    fn client_auth_method_selection() {
        let with =
            |methods: Value| discovery(json!({"token_endpoint_auth_methods_supported": methods}));
        assert!(uses_post_auth(&discovery(json!({}))));
        assert!(uses_post_auth(&with(json!([
            "client_secret_basic",
            "client_secret_post"
        ]))));
        assert!(uses_post_auth(&with(json!(["client_secret_post"]))));
        assert!(!uses_post_auth(&with(json!(["client_secret_basic"]))));
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
