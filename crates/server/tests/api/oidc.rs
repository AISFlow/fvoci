//! Signing in through an organization's `OpenID Connect` provider, against the fake
//! provider of the tests: the provider's settings, the first sign-in and the next, the
//! forgeries that are refused, the accounts that are not joined, and the defences of
//! the requests to the provider.

use base64::Engine as _;
use fvoci_model::{
    LoginOptions, NameShown, OidcProvider, OidcStart, User, UserWorkspace, Workspace, new_id,
};
use fvoci_server::app::secret_context;
use fvoci_server::auth::sealed::SecretKey;
use fvoci_server::auth::secret::token_digest;
use fvoci_server::config::Configuration;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, StoredProvider};
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, USER1_USERNAME};
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Forgery, Person};

/// The key the test servers seal with.
pub(crate) fn secret_key() -> SecretKey {
    SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([42_u8; 32]))
        .expect("a key")
}

pub(crate) fn with_key_and_private_addresses(config: &mut Configuration) {
    config.secret_key = Some(secret_key());
    config.oidc_allow_private_addresses = true;
}

fn with_key_only(config: &mut Configuration) {
    config.secret_key = Some(secret_key());
}

/// A server that reaches the fake provider, with `user1`, who administers the
/// organization, and `user2`, a member.
pub(crate) async fn server_for(fake: &FakeProvider) -> TestServer {
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    ts.app
        .store()
        .system()
        .execute_unchecked("UPDATE users SET email_verified_at = created_at WHERE email <> ''")
        .await
        .expect("fixture accounts have verified addresses");
    ts
}

/// The organization of the test server.
pub(crate) async fn org_of(ts: &TestServer) -> String {
    let workspaces: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    workspaces
        .first()
        .expect("user1 is in a workspace")
        .org_id
        .clone()
}

/// Whether the organization lets newcomers in through its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Newcomers {
    Welcome,
    Refused,
}

/// The settings of the fake provider, with the client secret.
fn settings(fake: &FakeProvider, newcomers: Newcomers) -> serde_json::Value {
    json!({
        "issuer": fake.issuer,
        "clientId": CLIENT_ID,
        "clientSecret": CLIENT_SECRET,
        "autoJoin": newcomers == Newcomers::Welcome,
    })
}

/// Sets the fake provider as the organization's.
pub(crate) async fn set_provider(
    ts: &TestServer,
    org_id: &str,
    fake: &FakeProvider,
) -> OidcProvider {
    ts.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &settings(fake, Newcomers::Welcome),
        )
        .await
        .ok_json()
}

/// What a sign-in came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A session, with its token.
    SignedIn(String),
    /// The login page, with the name of what went wrong.
    Refused(String),
}

impl Outcome {
    pub(crate) fn token(&self) -> &str {
        match self {
            Self::SignedIn(token) => token,
            Self::Refused(name) => panic!("the sign-in was refused: {name}"),
        }
    }
}

/// A browser: it follows no redirect by itself, keeps the cookie of a sign-in, and
/// trusts the fake provider's certificate.
pub(crate) struct Browser {
    http: reqwest::Client,
    base_url: String,
    server_url: String,
}

/// A sign-in on its way back from the provider: the path of the server it comes back
/// to, and the browser's cookie.
#[derive(Debug, Clone)]
struct Returning {
    path: String,
    cookie: Option<String>,
}

impl Browser {
    pub(crate) fn new(ts: &TestServer, fake: &FakeProvider) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_merge([
                reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
            ])
            .build()
            .expect("a client");
        Self {
            http,
            base_url: ts.base_url.clone(),
            server_url: ts.app.config().server_url.clone(),
        }
    }

    /// Starts a sign-in for the organization as the login page does, and lets the
    /// provider answer: the way back to the server, or the name of the problem the
    /// start was refused with.
    async fn go_to_provider(&self, org_id: &str) -> Result<Returning, String> {
        let started = self
            .http
            .post(format!(
                "{}/api/v1/auth/oidc/start?org={org_id}",
                self.base_url
            ))
            .header("X-Requested-With", "XMLHttpRequest")
            .send()
            .await
            .expect("the server answers");
        let cookie = started
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie| cookie.split(';').next()?.strip_prefix("fvoci_oidc="))
            .map(str::to_owned);
        if !started.status().is_success() {
            let problem: serde_json::Value = started.json().await.expect("a problem");
            let name = problem["type"].as_str().expect("its type");
            assert!(cookie.is_none(), "a refused start sets no cookie");
            return Err(name.trim_start_matches("/problems/").to_owned());
        }
        let started: OidcStart = started.json().await.expect("where the browser goes");
        let authorized = self
            .http
            .get(&started.authorization_url)
            .send()
            .await
            .expect("the provider answers");
        assert_eq!(
            authorized.status(),
            StatusCode::FOUND,
            "the provider sends back"
        );
        let back = location_of(&authorized);
        let path = back
            .strip_prefix(self.server_url.trim_end_matches('/'))
            .unwrap_or_else(|| panic!("{back} is under the server's address"))
            .to_owned();
        assert!(
            path.starts_with("/api/v1/auth/oidc/callback/"),
            "back to the organization's address: {path}"
        );
        Ok(Returning { path, cookie })
    }

    /// Comes back to the server.
    async fn come_back(&self, returning: &Returning) -> Outcome {
        let mut request = self
            .http
            .get(format!("{}{}", self.base_url, returning.path));
        if let Some(cookie) = &returning.cookie {
            request = request.header("Cookie", format!("fvoci_oidc={cookie}"));
        }
        let answer = request.send().await.expect("the server answers");
        assert_eq!(answer.status(), StatusCode::SEE_OTHER, "a return sends on");
        let location = location_of(&answer);
        let cleared = answer
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|cookie| cookie.starts_with("fvoci_oidc=;") && cookie.contains("Max-Age=0"));
        assert!(cleared, "the cookie of the sign-in is cleared");
        if let Some(name) = refusal_of(&location) {
            return Outcome::Refused(name);
        }
        assert_eq!(
            location,
            format!("{}/", self.server_url.trim_end_matches('/')),
            "to the web app"
        );
        let token = answer
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie| cookie.split(';').next()?.strip_prefix("fvoci_session="))
            .expect("the session cookie is set")
            .to_owned();
        Outcome::SignedIn(token)
    }

    pub(crate) async fn sign_in(&self, org_id: &str) -> Outcome {
        match self.go_to_provider(org_id).await {
            Ok(returning) => self.come_back(&returning).await,
            Err(name) => Outcome::Refused(name),
        }
    }
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location header")
        .to_owned()
}

/// The name in `oidc_error` of an address of the login page; `None` for another
/// address.
fn refusal_of(location: &str) -> Option<String> {
    let (_, query) = location.split_once("/login?")?;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("oidc_error="))
        .map(str::to_owned)
}

async fn identities(ts: &TestServer) -> i64 {
    ts.app
        .store()
        .count_rows("user_identities")
        .await
        .expect("the identities are counted")
}

/// The addresses the providers gave for an account.
async fn provider_emails(ts: &TestServer, user_id: &str) -> Vec<String> {
    ts.app
        .store()
        .provider_emails(user_id)
        .await
        .expect("the identities are read")
}

async fn users(ts: &TestServer) -> i64 {
    ts.app
        .store()
        .count_rows("users")
        .await
        .expect("the users are counted")
}

/// The first sign-in makes an account with no password, in the organization and its
/// default workspace, and opens a session made through the provider; the next sign-in
/// is into the same account.
#[tokio::test]
async fn the_first_sign_in_makes_an_account_and_the_next_finds_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let provider = set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);

    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let first = browser.sign_in(&org_id).await;
    let client = ts.client_with_token(first.token());
    let me = client.me().await;
    assert_eq!(
        (me.username.as_str(), me.display_name.as_str()),
        ("ada", "Person subject-ada")
    );
    let workspaces: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    assert!(
        workspaces
            .iter()
            .any(|workspace| workspace.id == ROOT_WORKSPACE_ID && workspace.org_id == org_id),
        "a member of the default workspace: {workspaces:?}"
    );
    let own = crate::own_space::own_workspace(&client).await;
    assert_ne!(own.org_id, org_id, "and the account has its own space");
    let stored = ts
        .app
        .store()
        .get_user_by_id(&me.id)
        .await
        .expect("the account is stored");
    assert_eq!(stored.email, "", "no address of its own");
    assert_eq!(
        provider_emails(&ts, &me.id).await,
        ["ada@example.com"],
        "the provider's word, kept with the identity"
    );
    assert_eq!(stored.password_hash, "", "no password");
    assert!(!stored.is_admin, "no administrator of the server");
    let session = ts
        .app
        .store()
        .get_session(&token_digest(first.token()), 0, 0)
        .await
        .expect("the session is stored");
    let provider_id = session
        .login_provider_id()
        .expect("the session names its provider")
        .to_owned();
    assert!(fvoci_model::is_id(&provider_id), "{provider_id}");
    assert!(provider.enabled && provider.auto_join, "{provider:?}");

    let second = browser.sign_in(&org_id).await;
    assert_ne!(second.token(), first.token(), "a session of its own");
    let again = ts.client_with_token(second.token()).me().await;
    assert_eq!(again.id, me.id, "the same account");
    assert_eq!(identities(&ts).await, 1);

    // A password login is a session without a provider.
    let password_session = ts.login(USER1_USERNAME, PASSWORD).await;
    let session = ts
        .app
        .store()
        .get_session(&token_digest(&password_session.token), 0, 0)
        .await
        .expect("the session is stored");
    assert_eq!(session.login, None);
    ts.teardown().await;
    fake.stop();
}

/// An account that was made with no password is not signed in to by any password.
#[tokio::test]
async fn an_account_without_a_password_takes_no_password() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let mut person = Person::new("subject-nopw", "nopw@example.com");
    person.preferred_username = Some("nopw".to_owned());
    fake.sign_in_as(person);
    Browser::new(&ts, &fake).sign_in(&org_id).await.token();

    for password in [
        "",
        " ",
        "nopw",
        "Pa$$word",
        "$argon2id$v=19$m=19456,t=2,p=1$",
    ] {
        for name in ["nopw", "nopw@example.com"] {
            let refused = ts
                .anonymous()
                .post(
                    "/login",
                    &json!({ "username": name, "email": name, "password": password }),
                )
                .await;
            assert_eq!(
                refused.status,
                StatusCode::UNAUTHORIZED,
                "{name} {password:?}"
            );
            assert_eq!(refused.error_reason(), "invalid_credentials");
        }
    }
    ts.teardown().await;
    fake.stop();
}

/// A sign-in that does not come back as it went is refused, and makes no account: a
/// `state` that is none of the server's, one used twice, one that comes back to
/// another browser, and an ID token with another audience, issuer or nonce, that
/// expired, that another key signed, or that is not signed.
#[tokio::test]
async fn forgeries_are_refused() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(Person::new("subject-mallory", "mallory@example.com"));
    let users_before = users(&ts).await;

    // A state that is none of the server's.
    let returning = browser.go_to_provider(&org_id).await.expect("started");
    let (path, _) = returning.path.split_once("&state=").expect("a state");
    let wrong = Returning {
        path: format!("{path}&state={}", new_id()),
        cookie: returning.cookie.clone(),
    };
    assert_eq!(
        browser.come_back(&wrong).await,
        Outcome::Refused("oidc-state-invalid".to_owned()),
        "a state of nothing"
    );

    // Used twice: the second return finds nothing.
    let returning = browser.go_to_provider(&org_id).await.expect("started");
    let replayed = returning.clone();
    let first = browser.come_back(&returning).await;
    let first_token = first.token().to_owned();
    assert_eq!(
        browser.come_back(&replayed).await,
        Outcome::Refused("oidc-state-invalid".to_owned()),
        "a state used again"
    );
    ts.client_with_token(&first_token).me().await;

    // Another browser: without the cookie, or with another one. It takes nothing:
    // the browser that started the sign-in still ends it.
    for cookie in [None, Some(new_id())] {
        let returning = browser.go_to_provider(&org_id).await.expect("started");
        let mut other = returning.clone();
        other.cookie = cookie.clone();
        assert_eq!(
            browser.come_back(&other).await,
            Outcome::Refused("oidc-state-invalid".to_owned()),
            "the cookie {cookie:?}"
        );
        browser.come_back(&returning).await.token();
    }

    fake.sign_in_as(Person::new("subject-eve", "eve@example.com"));
    let users_now = users(&ts).await;
    for forgery in [
        Forgery::Audience,
        Forgery::AudienceNotText,
        Forgery::Issuer,
        Forgery::Expired,
        Forgery::Nonce,
        Forgery::OtherKey,
        Forgery::AlgNone,
        Forgery::UnknownCritical,
    ] {
        fake.forge(forgery);
        assert_eq!(
            browser.sign_in(&org_id).await,
            Outcome::Refused("oidc-token-invalid".to_owned()),
            "{forgery:?}"
        );
    }
    assert_eq!(users(&ts).await, users_now, "no account of eve");
    assert_eq!(
        users_now,
        users_before + 1,
        "mallory's, of the one that went through"
    );
    fake.forge(Forgery::None);
    browser.sign_in(&org_id).await.token();
    ts.teardown().await;
    fake.stop();
}

/// Somebody who signs in for the first time with the email address of an account, in
/// any letter case, is refused and not joined to it; an address the provider did not
/// check, and none, are refused too.
#[tokio::test]
async fn an_email_address_of_another_account_is_not_joined() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let users_before = users(&ts).await;

    for email in ["user1@sample.com", "USER1@Sample.com"] {
        fake.sign_in_as(Person::new(&format!("subject-{email}"), email));
        assert_eq!(
            browser.sign_in(&org_id).await,
            Outcome::Refused("oidc-email-taken".to_owned()),
            "{email}"
        );
    }
    let mut unchecked = Person::new("subject-unchecked", "unchecked@example.com");
    unchecked.email_verified = Some(false);
    fake.sign_in_as(unchecked);
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-email-unverified".to_owned())
    );
    // Omitted `email_verified` (`None`) is also refused rather than implicitly trusted (`H-8`).
    let mut omitted_verified = Person::new("subject-omitted-verified", "omitted@example.com");
    omitted_verified.email_verified = None;
    fake.sign_in_as(omitted_verified);
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-email-unverified".to_owned())
    );
    let mut none = Person::new("subject-none", "x@example.com");
    none.email = None;
    fake.sign_in_as(none);
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-email-missing".to_owned())
    );
    assert_eq!(users(&ts).await, users_before, "no account was made");
    assert_eq!(identities(&ts).await, 0, "and no identity");

    // An unverified local address entered while mail is disabled (`email_verified_at = 0`)
    // cannot squat and block a verified OIDC user's first sign-in (`H-3`).
    let squatter = ts
        .register_and_login("unverified-squatter", "ceo@victim.example", PASSWORD)
        .await
        .me()
        .await;
    fake.sign_in_as(Person::new("subject-real-ceo", "ceo@victim.example"));
    let real_ceo = browser.sign_in(&org_id).await;
    let ceo_me = ts.client_with_token(real_ceo.token()).me().await;
    assert_ne!(
        ceo_me.id, squatter.id,
        "verified OIDC sign-in succeeds as its own account despite unverified squatter"
    );

    // The account is untouched: its password still signs in.
    ts.login(USER1_USERNAME, PASSWORD).await;
    ts.teardown().await;
    fake.stop();
}

/// An organization that lets nobody in by its provider refuses whoever has no account
/// there, and lets in its members.
#[tokio::test]
async fn an_organization_closed_to_newcomers_refuses_them() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let closed: OidcProvider = ts
        .client
        .put(
            &format!("/org/{org_id}/oidc"),
            &settings(&fake, Newcomers::Refused),
        )
        .await
        .ok_json();
    assert!(!closed.auto_join);
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(Person::new("subject-new", "new@example.com"));
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-join-closed".to_owned())
    );
    assert_eq!(identities(&ts).await, 0);

    // Open, the person comes in; closed again, the member still does.
    set_provider(&ts, &org_id, &fake).await;
    browser.sign_in(&org_id).await.token();
    ts.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &settings(&fake, Newcomers::Refused),
        )
        .await
        .check_ok();
    browser.sign_in(&org_id).await.token();
    ts.teardown().await;
    fake.stop();
}

/// The provider's settings are for the organization's administrators: a member, a
/// stranger and an organization that is not there are told 404, a request without a
/// session 401.
#[tokio::test]
async fn only_an_administrator_of_the_organization_reaches_its_provider() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    ts.client.get(&path).await.check_not_found();
    set_provider(&ts, &org_id, &fake).await;
    ts.client.get(&path).await.check_ok();

    for client in [&ts.client2] {
        client.get(&path).await.check_not_found();
        client
            .put(&path, &settings(&fake, Newcomers::Welcome))
            .await
            .check_not_found();
        client.delete(&path).await.check_not_found();
    }
    ts.anonymous().get(&path).await.check_unauthorized();
    for other in [
        new_id(),
        "not-an-id".to_owned(),
        fvoci_store::SYSTEM_ORG_ID.to_owned(),
    ] {
        ts.client
            .get(&format!("/org/{other}/oidc"))
            .await
            .check_not_found();
    }
    ts.client.get(&path).await.check_ok();
    ts.client.delete(&path).await.check_no_content();
    ts.client.get(&path).await.check_not_found();
    ts.client.delete(&path).await.check_no_content();
    assert_eq!(
        Browser::new(&ts, &fake).sign_in(&org_id).await,
        Outcome::Refused("oidc-unavailable".to_owned()),
        "no provider, no sign-in"
    );
    ts.teardown().await;
    fake.stop();
}

/// The client secret is stored sealed, and no answer has it.
#[tokio::test]
async fn the_client_secret_is_sealed_and_never_answered() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let put = ts
        .client
        .put(&path, &settings(&fake, Newcomers::Welcome))
        .await;
    let got = ts.client.get(&path).await;
    for answer in [&put, &got] {
        assert!(!answer.text().contains(CLIENT_SECRET), "{}", answer.text());
        let provider: OidcProvider = answer.ok_json();
        assert!(provider.has_client_secret);
        assert_eq!(provider.client_id, CLIENT_ID);
        assert!(
            provider
                .redirect_uri
                .starts_with("http://localhost:8000/api/v1/auth/oidc/callback/"),
            "{}",
            provider.redirect_uri
        );
    }
    let stored: StoredProvider = ts
        .app
        .store()
        .for_org(&org_id)
        .expect("an identifier")
        .oidc_provider()
        .await
        .expect("read")
        .expect("stored");
    assert!(
        stored.sealed_client_secret.starts_with("v1.")
            && !stored.sealed_client_secret.contains(CLIENT_SECRET)
            && !stored.sealed_client_secret.contains("secret"),
        "sealed: {}",
        stored.sealed_client_secret
    );
    assert!(!format!("{stored:?}").contains(&stored.sealed_client_secret));

    // Left out, the secret stays; the sign-in uses it.
    let kept: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": CLIENT_ID }),
        )
        .await
        .ok_json();
    assert!(kept.has_client_secret);
    Browser::new(&ts, &fake).sign_in(&org_id).await.token();
    // The empty string stores none, and the provider refuses the client then.
    let none: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": "" }),
        )
        .await
        .ok_json();
    assert!(!none.has_client_secret);
    assert_eq!(
        Browser::new(&ts, &fake).sign_in(&org_id).await,
        Outcome::Refused("oidc-token-refused".to_owned())
    );
    ts.teardown().await;
    fake.stop();
}

/// Without a key to seal with, no provider is stored.
#[tokio::test]
async fn without_a_secret_key_no_provider_is_stored() {
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let refused = ts
        .client
        .put(
            &format!("/org/{org_id}/oidc"),
            &settings(&fake, Newcomers::Welcome),
        )
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.error_reason(), "secret_key_required");
    ts.client
        .get(&format!("/org/{org_id}/oidc"))
        .await
        .check_not_found();
    ts.teardown().await;
    fake.stop();
}

/// A provider at an address that is not public is neither stored nor signed in
/// through unless the configuration lets it be; the issuer must be what the provider
/// says it is; a redirect is not followed.
#[tokio::test]
async fn a_provider_is_reached_only_where_it_may_be() {
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_only)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let issuer = fake.issuer.clone();
    for (issuer, field_reason) in [
        (issuer.clone(), "oidc_issuer_unverified"),
        (
            issuer.replace("https://127.0.0.1", "https://localhost"),
            "oidc_issuer_unverified",
        ),
        (issuer.replace("https://", "http://"), "invalid_format"),
    ] {
        let refused = ts
            .client
            .put(
                &path,
                &json!({ "issuer": issuer, "clientId": CLIENT_ID, "clientSecret": "x" }),
            )
            .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{issuer}");
        assert_eq!(refused.refused_at(), "/issuer", "{issuer}");
        assert!(
            refused.text().contains(field_reason),
            "{issuer}: {}",
            refused.text()
        );
    }
    ts.client.get(&path).await.check_not_found();

    // Stored past the check, as by a server that let it, it is not signed in through.
    ts.app
        .store()
        .for_org(&org_id)
        .expect("an identifier")
        .put_oidc_provider(&StoredProvider {
            id: String::new(),
            org_id: org_id.clone(),
            issuer: fake.issuer.clone(),
            client_id: CLIENT_ID.to_owned(),
            sealed_client_secret: secret_key().seal(
                CLIENT_SECRET,
                &secret_context(&org_id, &fake.issuer, CLIENT_ID),
            ),
            auto_join: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            sso_required: false,
            revision: 0,
        })
        .await
        .expect("stored");
    assert_eq!(
        Browser::new(&ts, &fake).sign_in(&org_id).await,
        Outcome::Refused("oidc-unavailable".to_owned())
    );
    ts.teardown().await;

    // Allowed to reach it: an issuer that is not the provider's own, and a discovery
    // document that redirects, are refused.
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let other_issuer = format!("{}/", fake.issuer);
    let refused = ts
        .client
        .put(
            &path,
            &json!({ "issuer": other_issuer, "clientId": CLIENT_ID }),
        )
        .await;
    assert_eq!(refused.error_reason(), "oidc_issuer_unverified");
    assert!(
        refused.text().contains("another issuer"),
        "{}",
        refused.text()
    );
    fake.redirect_discovery();
    let refused = ts
        .client
        .put(&path, &settings(&fake, Newcomers::Welcome))
        .await;
    assert_eq!(refused.error_reason(), "oidc_issuer_unverified");
    assert!(refused.text().contains("status 302"), "{}", refused.text());
    ts.client.get(&path).await.check_not_found();
    ts.teardown().await;
    fake.stop();
}

/// The login page learns an organization's name and whether it has a provider: of the
/// one it names, or of the only one of the server; of nothing else.
#[tokio::test]
async fn the_login_page_learns_of_the_organization_it_names_only() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let options = |query: String| {
        let anonymous = ts.anonymous();
        async move {
            anonymous
                .get(&format!("/login-options{query}"))
                .await
                .ok_json::<LoginOptions>()
        }
    };
    let only = options(String::new())
        .await
        .organization
        .expect("the only one");
    assert_eq!((only.id.as_str(), only.oidc), (org_id.as_str(), false));
    set_provider(&ts, &org_id, &fake).await;
    let named = options(format!("?org={org_id}"))
        .await
        .organization
        .expect("the named one");
    assert_eq!((named.id.as_str(), named.oidc), (org_id.as_str(), true));
    for nothing in [
        new_id(),
        "x".to_owned(),
        fvoci_store::SYSTEM_ORG_ID.to_owned(),
    ] {
        assert_eq!(
            options(format!("?org={nothing}")).await.organization,
            None,
            "{nothing}"
        );
    }
    ts.teardown().await;
    fake.stop();
}

/// A sign-in started for one organization lets into that organization only, and its
/// session names that organization's provider.
#[tokio::test]
async fn a_sign_in_lets_into_the_organization_it_was_started_for() {
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let ours = org_of(&ts).await;
    set_provider(&ts, &ours, &fake).await;
    let theirs_workspace = new_id();
    let theirs = ts
        .app
        .store()
        .create_organization(
            "theirs",
            &Workspace {
                id: theirs_workspace.clone(),
                ..Workspace::default()
            },
        )
        .await
        .expect("a second organization");
    let their_provider = ts
        .app
        .store()
        .for_org(&theirs)
        .expect("an identifier")
        .put_oidc_provider(&StoredProvider {
            id: String::new(),
            org_id: theirs.clone(),
            issuer: fake.issuer.clone(),
            client_id: CLIENT_ID.to_owned(),
            sealed_client_secret: secret_key().seal(
                CLIENT_SECRET,
                &secret_context(&theirs, &fake.issuer, CLIENT_ID),
            ),
            auto_join: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            sso_required: false,
            revision: 0,
        })
        .await
        .expect("their provider");

    fake.sign_in_as(Person::new("subject-theirs", "theirs@example.com"));
    let browser = Browser::new(&ts, &fake);
    let token = browser.sign_in(&theirs).await.token().to_owned();
    let client = ts.client_with_token(&token);
    let (workspaces, _own) = crate::own_space::workspaces_and_own(&client).await;
    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| (workspace.org_id.as_str(), workspace.id.as_str()))
            .collect::<Vec<_>>(),
        vec![(theirs.as_str(), theirs_workspace.as_str())],
        "theirs only"
    );
    let session = ts
        .app
        .store()
        .get_session(&token_digest(&token), 0, 0)
        .await
        .expect("stored");
    assert_eq!(
        session.login_provider_id(),
        Some(their_provider.id.as_str())
    );

    // A secret sealed for one organization does not open for the other.
    ts.app
        .store()
        .for_org(&theirs)
        .expect("an identifier")
        .put_oidc_provider(&StoredProvider {
            sealed_client_secret: secret_key().seal(
                CLIENT_SECRET,
                &secret_context(&ours, &fake.issuer, CLIENT_ID),
            ),
            ..their_provider
        })
        .await
        .expect("stored");
    assert_eq!(
        browser.sign_in(&theirs).await,
        Outcome::Refused("oidc-unavailable".to_owned())
    );
    let me: User = client.me().await;
    assert_eq!(provider_emails(&ts, &me.id).await, ["theirs@example.com"]);
    ts.teardown().await;
    fake.stop();
}

/// Two organizations whose providers give the same address for somebody make two
/// accounts of them: an address a provider gives is its word, and joins nothing.
#[tokio::test]
async fn the_same_address_from_two_providers_is_two_accounts() {
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let ours_fake = FakeProvider::start().await;
    let theirs_fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&ours_fake.certificate_der)
        .trusting_oidc_provider(&theirs_fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let ours = org_of(&ts).await;
    set_provider(&ts, &ours, &ours_fake).await;
    let theirs = ts
        .app
        .store()
        .create_organization(
            "theirs",
            &Workspace {
                id: new_id(),
                ..Workspace::default()
            },
        )
        .await
        .expect("a second organization");
    ts.app
        .store()
        .for_org(&theirs)
        .expect("an identifier")
        .put_oidc_provider(&StoredProvider {
            id: String::new(),
            org_id: theirs.clone(),
            issuer: theirs_fake.issuer.clone(),
            client_id: CLIENT_ID.to_owned(),
            sealed_client_secret: secret_key().seal(
                CLIENT_SECRET,
                &secret_context(&theirs, &theirs_fake.issuer, CLIENT_ID),
            ),
            auto_join: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            sso_required: false,
            revision: 0,
        })
        .await
        .expect("their provider");

    let address = "someone@example.com";
    ours_fake.sign_in_as(Person::new("subject-1", address));
    theirs_fake.sign_in_as(Person::new("subject-1", address));
    let ours_token = Browser::new(&ts, &ours_fake).sign_in(&ours).await;
    let theirs_token = Browser::new(&ts, &theirs_fake).sign_in(&theirs).await;
    let ours_me = ts.client_with_token(ours_token.token()).me().await;
    let theirs_me = ts.client_with_token(theirs_token.token()).me().await;
    assert_ne!(ours_me.id, theirs_me.id, "two accounts");
    for me in [&ours_me, &theirs_me] {
        assert_eq!(provider_emails(&ts, &me.id).await, [address]);
    }
    assert_eq!(identities(&ts).await, 2);
    ts.teardown().await;
    ours_fake.stop();
    theirs_fake.stop();
}

/// An address a provider gave holds no place: its owner registers with it and a
/// password afterwards, and another person the same provider gives it for is another
/// account. The account the password made then refuses whoever comes new with it.
#[tokio::test]
async fn an_address_a_provider_gave_is_still_free_to_register() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let address = "owner@example.com";

    fake.sign_in_as(Person::new("subject-squatter", address));
    let squatter = browser.sign_in(&org_id).await;
    let squatter = ts.client_with_token(squatter.token()).me().await;
    fake.sign_in_as(Person::new("subject-other", &address.to_uppercase()));
    let other = browser.sign_in(&org_id).await;
    let other = ts.client_with_token(other.token()).me().await;
    assert_ne!(squatter.id, other.id, "two accounts of one address");

    let owner = ts
        .register_and_login("the-owner", address, PASSWORD)
        .await
        .me()
        .await;
    ts.app
        .store()
        .system()
        .execute_unchecked(&format!(
            "UPDATE users SET email_verified_at = created_at WHERE id = '{}'",
            owner.id
        ))
        .await
        .expect("verified owner address");
    assert_ne!(owner.id, squatter.id);
    ts.anonymous()
        .post("/login", &json!({ "email": address, "password": PASSWORD }))
        .await
        .check_no_content();
    let found = ts
        .app
        .store()
        .get_user_by_email(address)
        .await
        .expect("the address finds its owner");
    assert_eq!(found.id, owner.id, "and nobody else");

    fake.sign_in_as(Person::new("subject-late", address));
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-email-taken".to_owned())
    );
    // Those made before stay who they were.
    fake.sign_in_as(Person::new("subject-squatter", address));
    let again = browser.sign_in(&org_id).await;
    assert_eq!(
        ts.client_with_token(again.token()).me().await.id,
        squatter.id
    );
    // An account that is deleted is nobody's to sign in to and link: its address
    // refuses a newcomer no more (a registration still, as an address once used).
    ts.app
        .store()
        .system()
        .execute_unchecked(&format!(
            "UPDATE users SET deleted_at = 1 WHERE id = '{}'",
            owner.id
        ))
        .await
        .expect("the account is marked deleted");
    fake.sign_in_as(Person::new("subject-late", address));
    let late = browser.sign_in(&org_id).await;
    let late = ts.client_with_token(late.token()).me().await;
    assert!(![&squatter.id, &other.id, &owner.id].contains(&&late.id));
    ts.teardown().await;
    fake.stop();
}

/// A sign-in started by a page of another site is not started: such a page could send
/// a browser through a provider of its own choosing and back signed in to an account
/// of its choosing. The start is a request of the login page, behind the API's defence
/// against forgery, whatever the browser says of where it comes from; the page's own
/// request starts one.
#[tokio::test]
async fn a_sign_in_is_not_started_from_another_site() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let start = format!("{}/api/v1/auth/oidc/start?org={org_id}", browser.base_url);
    for site in ["cross-site", "same-site", "same-origin", "none"] {
        let navigated = browser
            .http
            .get(&start)
            .header("Sec-Fetch-Site", site)
            .send()
            .await
            .expect("an answer");
        assert_eq!(navigated.status(), StatusCode::METHOD_NOT_ALLOWED, "{site}");
        let posted = browser
            .http
            .post(&start)
            .header("Sec-Fetch-Site", site)
            .send()
            .await
            .expect("an answer");
        assert_eq!(
            posted.status(),
            StatusCode::FORBIDDEN,
            "{site}: no header of the page"
        );
        for answer in [&navigated, &posted] {
            assert!(
                answer.headers().get(reqwest::header::SET_COOKIE).is_none(),
                "{site}: no sign-in was started"
            );
        }
    }
    let returning = browser.go_to_provider(&org_id).await.expect("started");
    assert!(
        returning.cookie.is_some(),
        "the page's own request starts one"
    );
    ts.teardown().await;
    fake.stop();
}

/// A provider that is given another issuer or client keeps no secret it had: the secret
/// is asked for again, and the sign-ins started through the one before do not come
/// back through the new one.
#[tokio::test]
async fn a_secret_stays_with_its_issuer_and_client() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let first = set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let started = browser.go_to_provider(&org_id).await.expect("started");
    let provider_id = |ts: &TestServer| {
        let store = ts.app.store().for_org(&org_id).expect("an identifier");
        async move {
            store
                .oidc_provider()
                .await
                .expect("read")
                .expect("stored")
                .id
        }
    };
    let before = provider_id(&ts).await;

    let refused = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": "another-client" }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(refused.refused_at(), "/clientSecret");
    // Given anew, it is another provider; the sign-in started before ends with the
    // one it was started through.
    let other: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": "another-client", "clientSecret": "s" }),
        )
        .await
        .ok_json();
    assert!(other.has_client_secret && other.created_at >= first.created_at);
    assert_ne!(provider_id(&ts).await, before, "another provider");
    assert_eq!(
        browser.come_back(&started).await,
        Outcome::Refused("oidc-state-invalid".to_owned())
    );
    // Back to the first client, with its secret: the same provider then on.
    set_provider(&ts, &org_id, &fake).await;
    let again = provider_id(&ts).await;
    ts.client
        .put(&path, &settings(&fake, Newcomers::Refused))
        .await
        .check_ok();
    assert_eq!(provider_id(&ts).await, again, "the same issuer and client");
    ts.teardown().await;
    fake.stop();
}

/// An organization that is not in use lets nobody sign in through its provider.
#[tokio::test]
async fn an_organization_not_in_use_lets_nobody_in() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let started = browser.go_to_provider(&org_id).await.expect("started");
    ts.app
        .store()
        .system()
        .execute_unchecked(&format!(
            "UPDATE organizations SET deleted_at = 1 WHERE id = '{org_id}'"
        ))
        .await
        .expect("the organization is marked deleted");
    assert_eq!(
        browser.come_back(&started).await,
        Outcome::Refused("oidc-unavailable".to_owned()),
        "a sign-in under way"
    );
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-unavailable".to_owned()),
        "a new one"
    );
    assert_eq!(identities(&ts).await, 0);
    ts.teardown().await;
    fake.stop();
}

/// Sign-ins are counted against the address they come from, as logins are: past the
/// limit, a start is answered 429, the problem `rate-limited`, which the page tells as
/// too many tries.
#[tokio::test]
async fn too_many_sign_ins_from_one_address_are_refused() {
    fn with_few_sign_ins(config: &mut Configuration) {
        with_key_and_private_addresses(config);
        config.login_address_attempts_per_minute = 12;
    }
    let fake = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_few_sign_ins)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut refused = None;
    for _ in 0..13 {
        if let Err(refusal) = browser.go_to_provider(&org_id).await {
            refused = Some(refusal);
            break;
        }
    }
    assert_eq!(refused.as_deref(), Some("rate-limited"));
    ts.teardown().await;
    fake.stop();
}

/// The identity of an account that is deleted signs nobody in, and makes no account
/// anew.
#[tokio::test]
async fn the_identity_of_a_deleted_account_lets_nobody_in() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    fake.sign_in_as(Person::new("subject-gone", "gone@example.com"));
    let first = browser.sign_in(&org_id).await;
    let me = ts.client_with_token(first.token()).me().await;
    ts.app
        .store()
        .system()
        .execute_unchecked(&format!(
            "UPDATE users SET deleted_at = 1 WHERE id = '{}'",
            me.id
        ))
        .await
        .expect("the account is marked deleted");
    let users_before = users(&ts).await;
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-unavailable".to_owned())
    );
    assert_eq!(users(&ts).await, users_before, "no account was made");
    assert_eq!(identities(&ts).await, 1);
    ts.teardown().await;
    fake.stop();
}

/// A setting left out of a change of the provider stays as it is, as the secret does:
/// giving a new secret does not open an organization closed to newcomers, nor switch on
/// a provider that is off. A start that names no organization is a bad request.
#[tokio::test]
async fn what_a_change_leaves_out_stays() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let closed: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({
                "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET,
                "autoJoin": false, "enabled": false,
            }),
        )
        .await
        .ok_json();
    assert!(!closed.auto_join && !closed.enabled);
    let rotated: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": "rotated" }),
        )
        .await
        .ok_json();
    assert!(!rotated.auto_join && !rotated.enabled, "{rotated:?}");
    assert_eq!(
        rotated.redirect_uri, closed.redirect_uri,
        "the same provider"
    );
    // Another client is another provider; the address to register stays the
    // organization's, and what was left out stays too.
    let other: OidcProvider = ts
        .client
        .put(
            &path,
            &json!({ "issuer": fake.issuer, "clientId": "another-client", "clientSecret": "s" }),
        )
        .await
        .ok_json();
    assert_eq!(
        other.redirect_uri, closed.redirect_uri,
        "the organization's address"
    );
    assert!(
        other
            .redirect_uri
            .ends_with(&format!("/api/v1/auth/oidc/callback/{org_id}")),
        "{}",
        other.redirect_uri
    );
    assert!(!other.auto_join && !other.enabled, "{other:?}");

    let unnamed = reqwest::Client::new()
        .post(format!("{}/api/v1/auth/oidc/start", ts.base_url))
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
        .expect("an answer");
    assert_eq!(unnamed.status(), StatusCode::BAD_REQUEST);
    ts.teardown().await;
    fake.stop();
}

/// A subject longer than `OpenID Connect` lets one be (255 bytes) is no token of a
/// provider's: refused, and no account made; at the limit, it signs in. An email address
/// longer than an account's may be is none: a newcomer with it is refused as one without
/// an address, and somebody known signs in whatever address comes.
#[tokio::test]
async fn a_subject_too_long_is_refused_and_an_address_too_long_is_none() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let users_before = users(&ts).await;
    let long_subject = "s".repeat(256);
    let long_email = format!("{}@example.com", "a".repeat(243));
    let mut refusals = Vec::new();
    for person in [
        Person::new(&long_subject, "long-subject@example.com"),
        Person::new("subject-long-email", &long_email),
    ] {
        fake.sign_in_as(person);
        refusals.push(browser.sign_in(&org_id).await);
    }
    let users_after_refusals = users(&ts).await;
    fake.sign_in_as(Person::new(&"s".repeat(255), "at-the-limit@example.com"));
    let at_limit = browser.sign_in(&org_id).await;
    // Known already: a later address too long does not keep them out.
    fake.sign_in_as(Person::new(&"s".repeat(255), &long_email));
    let known_long_email = browser.sign_in(&org_id).await;
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        refusals,
        [
            Outcome::Refused("oidc-token-invalid".to_owned()),
            Outcome::Refused("oidc-email-missing".to_owned()),
        ]
    );
    assert_eq!(users_after_refusals, users_before, "no account made");
    assert!(matches!(at_limit, Outcome::SignedIn(_)), "{at_limit:?}");
    assert!(
        matches!(known_long_email, Outcome::SignedIn(_)),
        "{known_long_email:?}"
    );
}

/// The first sign-in fills the parts of the name from what the provider gives; a part
/// the provider gives that a person may not give is left empty. The account is shown by
/// the parts when both came through, else by the provider's whole name. A later
/// sign-in changes nothing of the name: it is the person's.
#[tokio::test]
async fn the_first_sign_in_takes_the_parts_of_the_name_and_later_ones_leave_them() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);

    let mut person = Person::new("subject-kim", "kim@example.com");
    person.given_name = Some(" 영환 ".to_owned());
    person.family_name = Some("김\u{202E}".to_owned());
    fake.sign_in_as(person);
    let first = browser.sign_in(&org_id).await;
    let me = ts.client_with_token(first.token()).me().await;
    assert_eq!(
        (
            me.name.first_name.as_str(),
            me.name.last_name.as_str(),
            me.name.custom_name.as_str(),
            me.display_name.as_str()
        ),
        ("영환", "", "Person subject-kim", "Person subject-kim"),
        "a family name that turns the text around is none, and half a name is not shown"
    );

    let mut whole = Person::new("subject-lee", "lee@example.com");
    whole.given_name = Some("서연".to_owned());
    whole.family_name = Some("이".to_owned());
    fake.sign_in_as(whole);
    let both = browser.sign_in(&org_id).await;
    let lee = ts.client_with_token(both.token()).me().await;
    assert_eq!(
        (lee.name.name_shown, lee.display_name.as_str()),
        (NameShown::Name, "이서연"),
        "both parts: shown by them"
    );

    let mut again = Person::new("subject-kim", "kim@example.com");
    again.given_name = Some("Other".to_owned());
    again.family_name = Some("Name".to_owned());
    fake.sign_in_as(again);
    let second = browser.sign_in(&org_id).await;
    let after = ts.client_with_token(second.token()).me().await;
    assert_eq!(after.id, me.id);
    assert_eq!(after.name, me.name, "the name stays the person's");
    ts.teardown().await;
}
