//! What the server asks of a provider, and how it checks the answers: the discovery
//! document, the exchange of a code for tokens, and the ID token.
//!
//! The signature of an ID token is checked by `jsonwebtoken` with aws-lc-rs; what is
//! decided here is which algorithms and which key may sign one, and what of its claims
//! must hold besides (`iss`, `aud`, `azp`, `exp`, `nbf`, `nonce`).

use std::fmt::Write as _;

use fvoci_model::MAXIMUM_EMAIL_BYTES;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use subtle::ConstantTimeEq as _;
use thiserror::Error;

use super::fetch::{FetchError, Fetcher};

/// The algorithms an ID token may be signed with: the asymmetric ones. Neither `none`
/// nor an algorithm keyed with the client secret is taken.
pub const ALLOWED_ALGORITHMS: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

/// How far the clocks of the server and the provider may be apart, in seconds.
const CLOCK_SKEW_SECONDS: u64 = 60;

/// The longest subject, in bytes (`OpenID Connect Core 1.0`, 2: 255 ASCII characters).
const MAXIMUM_SUBJECT_BYTES: usize = 255;

/// The path of the discovery document under the issuer.
const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// What went wrong with a provider. Each is said in the server's words: nothing of the
/// provider's answer is repeated.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderError {
    /// The provider could not be asked, or its answer gave nothing to go on.
    #[error("the provider: {0}")]
    Fetch(#[from] FetchError),
    /// The discovery document names another issuer than the one it was read for.
    #[error("the discovery document names another issuer")]
    IssuerMismatch,
    /// An endpoint of the discovery document is no `https` address.
    #[error("an endpoint of the discovery document is not https")]
    EndpointNotHttps,
    /// The provider gave no ID token for the code.
    #[error("the provider gave no ID token")]
    NoIdToken,
    /// The ID token is not one this server takes; the reason is for the log.
    #[error("the ID token is not valid: {0}")]
    InvalidIdToken(&'static str),
}

/// What the discovery document says that the server uses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Discovery {
    /// The issuer, as the provider names itself.
    pub issuer: String,
    /// Where a browser is sent to sign in.
    pub authorization_endpoint: String,
    /// Where a code is exchanged for tokens.
    pub token_endpoint: String,
    /// Where the keys that sign the ID tokens are.
    pub jwks_uri: String,
    /// How the token endpoint takes the client's credentials; `None` if the provider
    /// does not say, which means `client_secret_basic`.
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    /// Whether the provider says which issuer answered, in the `iss` parameter of the
    /// address it sends the browser back to (RFC 9207); then it must.
    #[serde(default)]
    pub authorization_response_iss_parameter_supported: bool,
}

/// Reads the discovery document of `issuer` and checks that it is the issuer's: that
/// it names the issuer exactly, and that its endpoints are `https`.
///
/// # Errors
///
/// Returns why the document is not to be used.
pub async fn discover(fetcher: &Fetcher, issuer: &str) -> Result<Discovery, ProviderError> {
    let url = format!("{}{DISCOVERY_PATH}", issuer.trim_end_matches('/'));
    let discovery: Discovery = fetcher.get_json(&url).await?;
    if discovery.issuer != issuer {
        return Err(ProviderError::IssuerMismatch);
    }
    for endpoint in [
        &discovery.authorization_endpoint,
        &discovery.token_endpoint,
        &discovery.jwks_uri,
    ] {
        if !endpoint.starts_with("https://") {
            return Err(ProviderError::EndpointNotHttps);
        }
    }
    Ok(discovery)
}

/// The client this server is at a provider.
#[derive(Clone, Copy)]
pub struct Client<'a> {
    /// The client identifier.
    pub id: &'a str,
    /// The client secret; `None` for a client without one.
    pub secret: Option<&'a str>,
}

impl std::fmt::Debug for Client<'_> {
    /// Leaves out the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What a sign-in that comes back brings to exchange.
#[derive(Debug, Clone, Copy)]
pub struct Grant<'a> {
    /// The authorization code.
    pub code: &'a str,
    /// The address the browser was sent back to.
    pub redirect_uri: &'a str,
    /// The PKCE code verifier.
    pub verifier: &'a str,
}

#[derive(Deserialize)]
struct TokenAnswer {
    #[serde(default)]
    id_token: Option<String>,
}

/// Exchanges a code for tokens and returns the ID token, not yet checked.
///
/// The client authenticates with HTTP Basic (`client_secret_basic`) unless the provider
/// takes only `client_secret_post`; a client without a secret sends its identifier.
///
/// # Errors
///
/// Returns why the provider gave no ID token.
pub async fn exchange_code(
    fetcher: &Fetcher,
    discovery: &Discovery,
    client: Client<'_>,
    grant: Grant<'_>,
) -> Result<String, ProviderError> {
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", grant.code),
        ("redirect_uri", grant.redirect_uri),
        ("code_verifier", grant.verifier),
    ];
    let encoded = client
        .secret
        .filter(|_| takes_basic(discovery))
        .map(|secret| (form_encode(client.id), form_encode(secret)));
    let basic = encoded
        .as_ref()
        .map(|(user, password)| (user.as_str(), password.as_str()));
    if basic.is_none() {
        form.push(("client_id", client.id));
        if let Some(secret) = client.secret {
            form.push(("client_secret", secret));
        }
    }
    let answer: TokenAnswer = fetcher
        .post_form(&discovery.token_endpoint, &form, basic)
        .await?;
    answer.id_token.ok_or(ProviderError::NoIdToken)
}

/// Whether the token endpoint takes the client's credentials with HTTP Basic.
fn takes_basic(discovery: &Discovery) -> bool {
    match &discovery.token_endpoint_auth_methods_supported {
        None => true,
        Some(methods) => {
            methods.iter().any(|method| method == "client_secret_basic")
                || !methods.iter().any(|method| method == "client_secret_post")
        }
    }
}

/// `application/x-www-form-urlencoded` of one value: what the user and the password of
/// HTTP Basic are made of first (RFC 6749 2.3.1).
fn form_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push('+'),
            other => {
                // Writing to a `String` cannot fail.
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// The set of keys of a provider, as read. Keys this server cannot read are left out
/// rather than failing the set: a provider may list keys of other kinds.
#[derive(Debug, Clone, Default)]
pub struct Keys(Vec<Jwk>);

#[derive(Deserialize)]
struct RawKeys {
    keys: Vec<serde_json::Value>,
}

/// Reads the keys at the discovery document's `jwks_uri`. They are read anew for every
/// sign-in, so a provider that turns to new keys is followed at once.
///
/// # Errors
///
/// Returns why the keys could not be read.
pub async fn keys(fetcher: &Fetcher, discovery: &Discovery) -> Result<Keys, ProviderError> {
    let raw: RawKeys = fetcher.get_json(&discovery.jwks_uri).await?;
    Ok(Keys(
        raw.keys
            .into_iter()
            .filter_map(|key| serde_json::from_value(key).ok())
            .collect(),
    ))
}

/// What the server takes from an ID token that holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdClaims {
    /// The issuer the token names (`iss`), which is the provider's.
    pub issuer: String,
    /// The subject: who the person is at the issuer.
    pub subject: String,
    /// The email address, if the token has one no longer than an account's may be.
    pub email: Option<String>,
    /// Whether the provider says it checked the address; `None` if it does not say.
    pub email_verified: Option<bool>,
    /// The person's name, if the token has one.
    pub name: Option<String>,
    /// The person's given name, if the token has one.
    pub given_name: Option<String>,
    /// The person's family name, if the token has one.
    pub family_name: Option<String>,
    /// The name the person goes by at the provider, if the token has one.
    pub preferred_username: Option<String>,
}

#[derive(Deserialize)]
struct RawClaims {
    iss: String,
    sub: String,
    aud: serde_json::Value,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "bool_or_text")]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    given_name: Option<String>,
    #[serde(default)]
    family_name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
}

/// A boolean claim that some providers send as the text `"true"` or `"false"`.
fn bool_or_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::Bool(value)) => Some(value),
            Some(serde_json::Value::String(text)) if text == "true" => Some(true),
            Some(serde_json::Value::String(text)) if text == "false" => Some(false),
            _ => None,
        },
    )
}

/// What an ID token must answer to.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    /// The issuer of the provider.
    pub issuer: &'a str,
    /// The client identifier, which must be the audience.
    pub client_id: &'a str,
    /// The nonce of the sign-in.
    pub nonce: &'a str,
}

/// Checks an ID token: signed with an allowed algorithm by a key of `keys` that is for
/// signing and for that algorithm, issued by the issuer for this client (and, with
/// several audiences, authorized for it), not expired and not before its time, and
/// carrying the nonce of the sign-in.
///
/// # Errors
///
/// Returns [`ProviderError::InvalidIdToken`] with the reason, for the log.
pub fn verify_id_token(
    token: &str,
    keys: &Keys,
    expected: Expected<'_>,
) -> Result<IdClaims, ProviderError> {
    let invalid = ProviderError::InvalidIdToken;
    let header = jsonwebtoken::decode_header(token).map_err(|_| invalid("unreadable header"))?;
    if !ALLOWED_ALGORITHMS.contains(&header.alg) {
        return Err(invalid("algorithm not allowed"));
    }
    let key = signing_key(keys, header.alg, header.kid.as_deref())?;
    let key = DecodingKey::from_jwk(key).map_err(|_| invalid("unreadable key"))?;

    let mut validation = Validation::new(header.alg);
    validation.set_issuer(&[expected.issuer]);
    validation.set_audience(&[expected.client_id]);
    validation.leeway = CLOCK_SKEW_SECONDS;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let claims = jsonwebtoken::decode::<RawClaims>(token, &key, &validation)
        .map_err(|error| invalid(validation_reason(error.kind())))?
        .claims;

    // With several audiences the token must be for this client in particular.
    let audiences = match &claims.aud {
        serde_json::Value::Array(audiences) => audiences.len(),
        _ => 1,
    };
    if (audiences > 1 || claims.azp.is_some()) && claims.azp.as_deref() != Some(expected.client_id)
    {
        return Err(invalid("authorized party is another client"));
    }
    let nonce_matches = claims
        .nonce
        .as_deref()
        .is_some_and(|nonce| bool::from(nonce.as_bytes().ct_eq(expected.nonce.as_bytes())));
    if !nonce_matches {
        return Err(invalid("nonce does not match"));
    }
    if claims.sub.is_empty() {
        return Err(invalid("no subject"));
    }
    // `OpenID Connect Core 1.0`, 2: a subject is at most 255 ASCII characters. Longer,
    // it is no token's of a provider (and would not fit the key it is kept under).
    if claims.sub.len() > MAXIMUM_SUBJECT_BYTES {
        return Err(invalid("subject longer than 255 bytes"));
    }
    Ok(IdClaims {
        issuer: claims.iss,
        subject: claims.sub,
        // An address longer than an account's may be is none: it is kept nowhere, and a
        // newcomer without one is refused as one without an address.
        email: claims
            .email
            .filter(|email| email.trim().len() <= MAXIMUM_EMAIL_BYTES),
        email_verified: claims.email_verified,
        name: claims.name,
        given_name: claims.given_name,
        family_name: claims.family_name,
        preferred_username: claims.preferred_username,
    })
}

/// The key that signed a token of `alg` with the key identifier `kid`: the one with
/// that identifier, or without one the only key of the set for that algorithm.
fn signing_key<'k>(
    keys: &'k Keys,
    alg: Algorithm,
    kid: Option<&str>,
) -> Result<&'k Jwk, ProviderError> {
    let invalid = ProviderError::InvalidIdToken;
    let fits = |key: &&Jwk| {
        let for_signing = key
            .common
            .public_key_use
            .as_ref()
            .is_none_or(|key_use| *key_use == PublicKeyUse::Signature);
        let for_alg = key
            .common
            .key_algorithm
            .is_none_or(|key_alg| key_alg.to_string().parse::<Algorithm>().ok() == Some(alg));
        for_signing && for_alg && is_of_family(&key.algorithm, alg)
    };
    let mut candidates = keys.0.iter().filter(fits).filter(|key| match kid {
        Some(kid) => key.common.key_id.as_deref() == Some(kid),
        None => true,
    });
    let key = candidates
        .next()
        .ok_or(invalid("no key of the provider fits"))?;
    if kid.is_none() && candidates.next().is_some() {
        return Err(invalid("no key identifier, and more than one key fits"));
    }
    Ok(key)
}

/// Whether a key is of the kind an algorithm signs with: RSA for `RS…` and `PS…`, an
/// elliptic curve for `ES…`, an octet key pair for `EdDSA`.
fn is_of_family(key: &AlgorithmParameters, alg: Algorithm) -> bool {
    match key {
        AlgorithmParameters::RSA(_) => matches!(
            alg,
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512
        ),
        AlgorithmParameters::EllipticCurve(_) => {
            matches!(alg, Algorithm::ES256 | Algorithm::ES384)
        }
        AlgorithmParameters::OctetKeyPair(_) => alg == Algorithm::EdDSA,
        // A symmetric key, and kinds a later version may know: none signs an ID token
        // this server takes.
        _ => false,
    }
}

/// The reason, for the log, of a refusal of `jsonwebtoken`.
fn validation_reason(kind: &jsonwebtoken::errors::ErrorKind) -> &'static str {
    use jsonwebtoken::errors::ErrorKind;
    match kind {
        ErrorKind::InvalidSignature => "signature does not verify",
        ErrorKind::ExpiredSignature => "expired",
        ErrorKind::ImmatureSignature => "not valid yet",
        ErrorKind::InvalidIssuer => "another issuer",
        ErrorKind::InvalidAudience => "another audience",
        ErrorKind::InvalidAlgorithm => "algorithm does not fit the key",
        ErrorKind::MissingRequiredClaim(_) => "a required claim is missing",
        _ => "unreadable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_encoding_leaves_the_unreserved_characters_and_escapes_the_rest() {
        assert_eq!(form_encode("abc-._*09"), "abc-._*09");
        assert_eq!(form_encode("a b:c/d%é"), "a+b%3Ac%2Fd%25%C3%A9");
    }

    #[test]
    fn basic_authentication_is_used_unless_the_provider_takes_post_only() {
        let with = |methods: Option<&[&str]>| Discovery {
            issuer: String::new(),
            authorization_endpoint: String::new(),
            token_endpoint: String::new(),
            jwks_uri: String::new(),
            token_endpoint_auth_methods_supported: methods
                .map(|methods| methods.iter().map(|method| (*method).to_owned()).collect()),
            authorization_response_iss_parameter_supported: false,
        };
        assert!(takes_basic(&with(None)));
        assert!(takes_basic(&with(Some(&["client_secret_basic"]))));
        assert!(takes_basic(&with(Some(&[
            "client_secret_post",
            "client_secret_basic"
        ]))));
        assert!(!takes_basic(&with(Some(&["client_secret_post"]))));
        assert!(takes_basic(&with(Some(&["private_key_jwt"]))));
    }

    #[test]
    fn without_a_key_identifier_the_one_key_of_the_algorithm_s_kind_signs() {
        let keys =
            |json: serde_json::Value| -> Keys { Keys(serde_json::from_value(json).expect("keys")) };
        let rsa = serde_json::json!({ "kty": "RSA", "n": "sXch", "e": "AQAB" });
        let ec = serde_json::json!({
            "kty": "EC", "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        });
        let both = keys(serde_json::json!([rsa, ec]));
        assert!(matches!(
            signing_key(&both, Algorithm::RS256, None),
            Ok(key) if matches!(key.algorithm, AlgorithmParameters::RSA(_))
        ));
        assert!(matches!(
            signing_key(&both, Algorithm::ES256, None),
            Ok(key) if matches!(key.algorithm, AlgorithmParameters::EllipticCurve(_))
        ));
        assert!(
            signing_key(&both, Algorithm::EdDSA, None).is_err(),
            "no such key"
        );
        let two_rsa = keys(serde_json::json!([rsa, rsa]));
        assert!(
            signing_key(&two_rsa, Algorithm::RS256, None).is_err(),
            "which of two is not guessed"
        );
    }

    #[test]
    fn a_token_that_is_not_signed_with_an_allowed_algorithm_is_refused_before_any_key() {
        use base64::Engine as _;
        let encode = |json: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        let expected = Expected {
            issuer: "https://idp.example.com",
            client_id: "client",
            nonce: "n",
        };
        for header in [
            r#"{"alg":"none"}"#,
            r#"{"alg":"HS256"}"#,
            r#"{"alg":"None"}"#,
        ] {
            let token = format!("{}.{}.", encode(header), encode(r#"{"sub":"x"}"#));
            let refused = verify_id_token(&token, &Keys::default(), expected).expect_err(header);
            assert!(
                matches!(
                    refused,
                    ProviderError::InvalidIdToken("algorithm not allowed" | "unreadable header")
                ),
                "{header}: {refused}"
            );
        }
    }
}
