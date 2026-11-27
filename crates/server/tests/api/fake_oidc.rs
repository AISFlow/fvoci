//! A small `OpenID Connect` provider for the tests: discovery, authorization, token and
//! keys, over TLS with a certificate of its own, signing with keys it makes.
//!
//! The person it signs in, and what it gets wrong on purpose (a [`Forgery`]), are the
//! test's to set. It lets anybody in who asks, as a provider whose user is signed in
//! already does. It checks what a provider checks of a code: that it is used once, by
//! the client it was given to, for the same `redirect_uri`, with the PKCE verifier of
//! its challenge. The example `fake_oidc` runs it as a program, for the tests of the web
//! app.
#![allow(
    dead_code,
    reason = "shared with the example program, which uses a part of it"
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::{KeyPair as _, RSA_PKCS1_SHA256};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::server::TlsStream;

/// The client this server is at the fake provider.
pub(crate) const CLIENT_ID: &str = "fvoci-test-client";
/// Its secret.
pub(crate) const CLIENT_SECRET: &str = "fvoci test secret: not one to keep";
/// Its other secret, taken as well, as a provider that rotates its client's secrets
/// takes the next one beside the one before.
pub(crate) const NEXT_CLIENT_SECRET: &str = "fvoci test secret: the next one";

/// The person the provider signs in.
#[derive(Debug, Clone)]
pub(crate) struct Person {
    pub(crate) subject: String,
    pub(crate) email: Option<String>,
    pub(crate) email_verified: Option<bool>,
    pub(crate) name: Option<String>,
    pub(crate) given_name: Option<String>,
    pub(crate) family_name: Option<String>,
    pub(crate) preferred_username: Option<String>,
}

impl Person {
    /// A person with a subject, an address that is checked, and a name.
    pub(crate) fn new(subject: &str, email: &str) -> Self {
        Self {
            subject: subject.to_owned(),
            email: Some(email.to_owned()),
            email_verified: Some(true),
            name: Some(format!("Person {subject}")),
            given_name: None,
            family_name: None,
            preferred_username: None,
        }
    }
}

/// What the provider gets wrong in the next ID token, on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Forgery {
    /// Nothing.
    None,
    /// The audience is another client.
    Audience,
    /// The audience is a number, neither a text nor a list of texts.
    AudienceNotText,
    /// The issuer is another.
    Issuer,
    /// It expired an hour ago.
    Expired,
    /// The nonce is another.
    Nonce,
    /// It is signed with a key that is not among the provider's keys, under the
    /// identifier of one that is.
    OtherKey,
    /// It is not signed: `alg` is `none`.
    AlgNone,
    /// It is signed with the provider's key, and its header names an extension that
    /// must be understood (`crit`, RFC 7515 4.1.11) and is not.
    UnknownCritical,
}

/// What the provider says of who answered (RFC 9207).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnswerIssuer {
    /// It says nothing, and its discovery document does not say it would.
    Silent,
    /// Its discovery document says it sends `iss`, and it does.
    Says,
    /// Its discovery document says it sends `iss`, and it does not.
    Promises,
}

/// What the test sets and what the provider saw.
struct Settings {
    person: Person,
    forgery: Forgery,
    answer_issuer: AnswerIssuer,
    /// Answer the discovery document with a redirect elsewhere.
    discovery_redirects: bool,
    codes: HashMap<String, Grant>,
    /// The token requests that came, whatever came of them.
    token_requests: usize,
}

/// What a code was given for.
#[derive(Debug, Clone)]
struct Grant {
    client_id: String,
    redirect_uri: String,
    nonce: String,
    challenge: String,
}

struct Shared {
    issuer: String,
    key: KeyPair,
    other_key: KeyPair,
    state: Mutex<Settings>,
}

/// A running fake provider.
pub(crate) struct FakeProvider {
    /// Its issuer: `https://127.0.0.1:<port>`.
    pub(crate) issuer: String,
    /// Its certificate (DER), for a client to trust.
    pub(crate) certificate_der: Vec<u8>,
    /// The same certificate in PEM.
    pub(crate) certificate_pem: String,
    shared: Arc<Shared>,
    serving: tokio::task::JoinHandle<()>,
}

impl FakeProvider {
    /// Starts a provider on a free port of the loopback interface.
    pub(crate) async fn start() -> Self {
        Self::start_on(0).await
    }

    /// Starts a provider on `port` of the loopback interface (0 for a free one).
    pub(crate) async fn start_on(port: u16) -> Self {
        let tcp = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("a port on the loopback interface is free");
        let address = tcp.local_addr().expect("the listener has an address");
        let issuer = format!("https://{address}");
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()])
                .expect("a certificate is made");
        let certificate_der = cert.der().to_vec();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("the TLS versions")
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into()),
            )
            .expect("the certificate and its key go together");
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];

        let shared = Arc::new(Shared {
            issuer: issuer.clone(),
            key: KeyPair::generate(KeySize::Rsa2048).expect("a key is made"),
            other_key: KeyPair::generate(KeySize::Rsa2048).expect("a key is made"),
            state: Mutex::new(Settings {
                person: Person::new("subject-1", "person@example.com"),
                forgery: Forgery::None,
                answer_issuer: AnswerIssuer::Silent,
                discovery_redirects: false,
                codes: HashMap::new(),
                token_requests: 0,
            }),
        });
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/jwks", get(jwks))
            .route("/person", post(set_person))
            .with_state(Arc::clone(&shared));
        let listener = Tls {
            tcp,
            acceptor: TlsAcceptor::from(Arc::new(tls)),
        };
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            issuer,
            certificate_der,
            certificate_pem: cert.pem(),
            shared,
            serving,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Settings> {
        self.shared
            .state
            .lock()
            .expect("the state is never held across a panic")
    }

    /// Signs in `person` from now on.
    pub(crate) fn sign_in_as(&self, person: Person) {
        self.state().person = person;
    }

    /// Gets `forgery` wrong in the ID tokens from now on.
    pub(crate) fn forge(&self, forgery: Forgery) {
        self.state().forgery = forgery;
    }

    /// Says, or promises, who answered, from now on.
    pub(crate) fn answer_issuer(&self, answer: AnswerIssuer) {
        self.state().answer_issuer = answer;
    }

    /// Answers the discovery document with a redirect from now on.
    pub(crate) fn redirect_discovery(&self) {
        self.state().discovery_redirects = true;
    }

    /// How many token requests came.
    pub(crate) fn token_requests(&self) -> usize {
        self.state().token_requests
    }

    /// Stops the provider.
    pub(crate) fn stop(self) {
        self.serving.abort();
    }
}

/// The TLS listener of the provider: one handshake at a time, which is enough here.
struct Tls {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl axum::serve::Listener for Tls {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((stream, peer)) = self.tcp.accept().await else {
                continue;
            };
            if let Ok(Ok(stream)) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.acceptor.accept(stream),
            )
            .await
            {
                return (stream, peer);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

async fn discovery(State(shared): State<Arc<Shared>>) -> Response {
    let issuer = &shared.issuer;
    let promises_issuer = shared
        .state
        .lock()
        .expect("the state is never held across a panic")
        .answer_issuer
        != AnswerIssuer::Silent;
    if shared
        .state
        .lock()
        .expect("the state is never held across a panic")
        .discovery_redirects
    {
        return (
            StatusCode::FOUND,
            [(header::LOCATION, "https://169.254.169.254/latest/meta-data")],
        )
            .into_response();
    }
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "authorization_response_iss_parameter_supported": promises_issuer,
    }))
    .into_response()
}

/// Who the provider is to sign in, as a test that reaches it as a program names them.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NamedPerson {
    subject: String,
    email: String,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Signs in the person of the body from now on: for the tests that reach the provider
/// as a program (`examples/fake_oidc.rs`), which cannot call
/// [`FakeProvider::sign_in_as`]. Without a `name` the person has the one
/// [`Person::new`] gives.
async fn set_person(
    State(shared): State<Arc<Shared>>,
    Json(named): Json<NamedPerson>,
) -> StatusCode {
    let mut person = Person::new(&named.subject, &named.email);
    person.preferred_username = named.preferred_username;
    if named.name.is_some() {
        person.name = named.name;
    }
    shared
        .state
        .lock()
        .expect("the state is never held across a panic")
        .person = person;
    StatusCode::NO_CONTENT
}

/// The value of a query parameter, percent-decoded.
fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| percent_decode(value))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = bytes.get(index + 1..index + 3)
            && let Ok(value) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or("zz"), 16)
        {
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_default()
}

async fn authorize(State(shared): State<Arc<Shared>>, RawQuery(query): RawQuery) -> Response {
    let query = query.unwrap_or_default();
    let get = |name: &str| query_value(&query, name).unwrap_or_default();
    if get("response_type") != "code" || get("code_challenge_method") != "S256" {
        return (StatusCode::BAD_REQUEST, "unsupported request").into_response();
    }
    let redirect_uri = get("redirect_uri");
    let code = URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{query}{}", rand_text())));
    shared
        .state
        .lock()
        .expect("the state is never held across a panic")
        .codes
        .insert(
            code.clone(),
            Grant {
                client_id: get("client_id"),
                redirect_uri: redirect_uri.clone(),
                nonce: get("nonce"),
                challenge: get("code_challenge"),
            },
        );
    let state = get("state");
    let mut location = format!("{redirect_uri}?code={code}&state={}", form_encode(&state));
    let says = shared
        .state
        .lock()
        .expect("the state is never held across a panic")
        .answer_issuer
        == AnswerIssuer::Says;
    if says {
        location = format!("{location}&iss={}", form_encode(&shared.issuer));
    }
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

fn rand_text() -> String {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes).expect("random bytes");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn form_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The user and the password of an `Authorization: Basic` header, form-decoded.
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (user, password) = decoded.split_once(':')?;
    Some((percent_decode(user), percent_decode(password)))
}

async fn token(State(shared): State<Arc<Shared>>, headers: HeaderMap, body: String) -> Response {
    let refused =
        |error: &str| (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))).into_response();
    let mut state = shared
        .state
        .lock()
        .expect("the state is never held across a panic");
    state.token_requests += 1;
    let get = |name: &str| query_value(&body, name).unwrap_or_default();
    let taken = basic_credentials(&headers).is_some_and(|(id, secret)| {
        id == CLIENT_ID && [CLIENT_SECRET, NEXT_CLIENT_SECRET].contains(&secret.as_str())
    });
    if !taken {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    if get("grant_type") != "authorization_code" {
        return refused("unsupported_grant_type");
    }
    // A code is used once, whatever comes of it.
    let Some(grant) = state.codes.remove(&get("code")) else {
        return refused("invalid_grant");
    };
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(get("code_verifier").as_bytes()));
    if grant.client_id != CLIENT_ID
        || grant.redirect_uri != get("redirect_uri")
        || grant.challenge != challenge
    {
        return refused("invalid_grant");
    }
    let id_token = id_token(&shared, &state.person, state.forgery, &grant.nonce);
    Json(json!({
        "access_token": rand_text(),
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": id_token,
    }))
    .into_response()
}

/// The seconds since the Unix epoch.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn id_token(shared: &Shared, person: &Person, forgery: Forgery, nonce: &str) -> String {
    let now = now();
    let mut claims = json!({
        "iss": shared.issuer,
        "sub": person.subject,
        "aud": CLIENT_ID,
        "iat": now,
        "exp": now + 300,
        "nonce": nonce,
    });
    for (name, value) in [
        ("email", person.email.clone().map(Value::from)),
        ("email_verified", person.email_verified.map(Value::from)),
        ("name", person.name.clone().map(Value::from)),
        ("given_name", person.given_name.clone().map(Value::from)),
        ("family_name", person.family_name.clone().map(Value::from)),
        (
            "preferred_username",
            person.preferred_username.clone().map(Value::from),
        ),
    ] {
        if let Some(value) = value {
            claims[name] = value;
        }
    }
    match forgery {
        Forgery::None | Forgery::OtherKey | Forgery::AlgNone | Forgery::UnknownCritical => {}
        Forgery::Audience => claims["aud"] = json!("another-client"),
        Forgery::AudienceNotText => claims["aud"] = json!(1234),
        Forgery::Issuer => claims["iss"] = json!("https://another.example.com"),
        Forgery::Expired => {
            claims["iat"] = json!(now - 7200);
            claims["exp"] = json!(now - 3600);
        }
        Forgery::Nonce => claims["nonce"] = json!("another nonce"),
    }
    let header = match forgery {
        Forgery::AlgNone => json!({ "alg": "none", "typ": "JWT" }),
        Forgery::UnknownCritical => json!({
            "alg": "RS256", "typ": "JWT", "kid": "key-1",
            "crit": ["urn:example:binding"], "urn:example:binding": true,
        }),
        Forgery::None
        | Forgery::Audience
        | Forgery::AudienceNotText
        | Forgery::Issuer
        | Forgery::Expired
        | Forgery::Nonce
        | Forgery::OtherKey => json!({ "alg": "RS256", "typ": "JWT", "kid": "key-1" }),
    };
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    if forgery == Forgery::AlgNone {
        return format!("{signing_input}.");
    }
    let key = if forgery == Forgery::OtherKey {
        &shared.other_key
    } else {
        &shared.key
    };
    let mut signature = vec![0_u8; key.public_modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signing_input.as_bytes(),
        &mut signature,
    )
    .expect("the token is signed");
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

async fn jwks(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let public = shared.key.public_key();
    Json(json!({
        "keys": [{
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": "key-1",
            "n": URL_SAFE_NO_PAD.encode(public.modulus().big_endian_without_leading_zero()),
            "e": URL_SAFE_NO_PAD.encode(public.exponent().big_endian_without_leading_zero()),
        }]
    }))
}
