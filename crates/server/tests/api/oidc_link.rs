//! Linking a provider to an account, against the fake provider of the tests: who may
//! start it and what it ends in, the forgeries that tie nothing, and removing an
//! identity again.

use fvoci_model::{LinkedIdentity, OidcStart, User};
use fvoci_server::auth::secret::token_digest;
use fvoci_store::Identity;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ApiResponse, Client, PASSWORD, ROOT_WORKSPACE_ID, TestServer, USER1_USERNAME};
use crate::fake_oidc::{FakeProvider, Person};
use crate::oidc::{Browser, Outcome, org_of, server_for, set_provider};
use crate::sso::{Passwords, admin_through_provider, require_sso, through_provider};

/// A sign-in to link on its way back from the provider.
struct Returning {
    /// The path of the server it comes back to.
    path: String,
    /// The cookie of the sign-in.
    flow: String,
}

/// Where the server sent the browser at the end of a sign-in to link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ended {
    /// To the web app, linked.
    Linked,
    /// To the login page, with the name of what went wrong.
    Refused(String),
}

/// A browser that links: it follows no redirect by itself and trusts the fake
/// provider's certificate.
pub(crate) struct Linker {
    http: reqwest::Client,
    base_url: String,
    server_url: String,
}

fn cookies_set(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::to_owned)
        .collect()
}

impl Linker {
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

    /// Asks the server to start a sign-in that links, as the page of `session` does.
    async fn ask(&self, org_id: &str, session: Option<&str>, password: &str) -> reqwest::Response {
        let mut request = self
            .http
            .post(format!(
                "{}/api/v1/auth/oidc/start?org={org_id}",
                self.base_url
            ))
            .header("X-Requested-With", "XMLHttpRequest")
            .json(&json!({ "linkPassword": password }));
        if let Some(token) = session {
            request = request.header("Cookie", format!("fvoci_session={token}"));
        }
        request.send().await.expect("the server answers")
    }

    /// Starts a sign-in that links from the session, and lets the provider answer.
    async fn go_to_provider(&self, org_id: &str, session: &str) -> Returning {
        let started = self.ask(org_id, Some(session), PASSWORD).await;
        assert_eq!(started.status(), StatusCode::OK, "the link starts");
        let flow = cookies_set(&started)
            .iter()
            .find_map(|cookie| {
                cookie
                    .split(';')
                    .next()?
                    .strip_prefix("fvoci_oidc=")
                    .map(str::to_owned)
            })
            .expect("the cookie of the sign-in");
        let started: OidcStart = started.json().await.expect("where the browser goes");
        let authorized = self
            .http
            .get(&started.authorization_url)
            .send()
            .await
            .expect("the provider answers");
        let back = authorized
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("the provider sends back");
        let path = back
            .strip_prefix(self.server_url.trim_end_matches('/'))
            .expect("under the server's address")
            .to_owned();
        Returning { path, flow }
    }

    /// Comes back to the server with the cookies given.
    async fn come_back(
        &self,
        returning: &Returning,
        flow: Option<&str>,
        session: Option<&str>,
    ) -> Ended {
        let cookies: Vec<String> = [
            flow.map(|flow| format!("fvoci_oidc={flow}")),
            session.map(|token| format!("fvoci_session={token}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut request = self
            .http
            .get(format!("{}{}", self.base_url, returning.path));
        if !cookies.is_empty() {
            request = request.header("Cookie", cookies.join("; "));
        }
        let answer = request.send().await.expect("the server answers");
        assert_eq!(answer.status(), StatusCode::SEE_OTHER, "a return sends on");
        assert!(
            !cookies_set(&answer)
                .iter()
                .any(|cookie| cookie.starts_with("fvoci_session=")),
            "a link sets no session"
        );
        let location = answer
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("a Location header");
        let root = self.server_url.trim_end_matches('/');
        if location == format!("{root}/?oidc_linked=1") {
            return Ended::Linked;
        }
        let refused = location
            .strip_prefix(&format!("{root}/login?"))
            .and_then(|query| {
                query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("oidc_error="))
            })
            .unwrap_or_else(|| panic!("to the web app or to the login page: {location}"));
        Ended::Refused(refused.to_owned())
    }

    /// Links from the session, in one browser, as the provider signs `person` in.
    pub(crate) async fn link(&self, org_id: &str, session: &str) -> Ended {
        let returning = self.go_to_provider(org_id, session).await;
        self.come_back(&returning, Some(&returning.flow), Some(session))
            .await
    }
}

async fn identities(client: &Client) -> Vec<LinkedIdentity> {
    client.get("/users/me/identities").await.ok_json()
}

async fn unlink(client: &Client, identity: &LinkedIdentity, password: &str) -> ApiResponse {
    client
        .delete_json(
            "/users/me/identities",
            &json!({
                "issuer": identity.issuer, "subject": identity.subject, "password": password,
            }),
        )
        .await
}

async fn flows(ts: &TestServer) -> i64 {
    ts.app
        .store()
        .scoped(fvoci_store::Tenant::System)
        .count_rows("oidc_login_flows")
        .await
        .expect("counted")
}

/// Somebody with a password, whom the provider refuses to make a second account for,
/// links the provider to their account, and it then signs in to that account.
#[tokio::test]
async fn a_password_account_links_the_provider_and_is_signed_in_to_by_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let user1: User = ts.client.me().await;
    fake.sign_in_as(Person::new("subject-of-user1", "user1@sample.com"));
    let browser = Browser::new(&ts, &fake);
    assert_eq!(
        browser.sign_in(&org_id).await,
        Outcome::Refused("oidc-email-taken".to_owned()),
        "the address is the account's: not joined by it"
    );
    assert_eq!(identities(&ts.client).await, []);

    let linker = Linker::new(&ts, &fake);
    assert_eq!(linker.link(&org_id, &ts.client.token).await, Ended::Linked);
    let tied = identities(&ts.client).await;
    assert_eq!(tied.len(), 1, "{tied:?}");
    assert_eq!(
        (
            tied[0].issuer.as_str(),
            tied[0].subject.as_str(),
            tied[0].email.as_str()
        ),
        (fake.issuer.as_str(), "subject-of-user1", "user1@sample.com")
    );
    assert_eq!(identities(&ts.client2).await, [], "nobody else's");
    assert_eq!(flows(&ts).await, 0, "the sign-in is taken");

    // The provider signs in to the same account, in a session made through it.
    let token = browser.sign_in(&org_id).await.token().to_owned();
    let through = ts.client_with_token(&token);
    assert_eq!(through.me().await.id, user1.id, "the same account");
    let session = ts
        .app
        .store()
        .get_session(&token_digest(&token), 0, 0)
        .await
        .expect("stored");
    assert!(session.login.is_some(), "made through it");
    // Linked again, it is the account's already: as it was.
    assert_eq!(linker.link(&org_id, &ts.client.token).await, Ended::Linked);
    assert_eq!(identities(&ts.client).await, tied);
    ts.teardown().await;
    fake.stop();
}

/// A link is started by a session, with the account's password: not without one, not
/// with a wrong one, and not for an account that has none.
#[tokio::test]
async fn a_link_is_started_with_a_session_and_the_accounts_password() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let linker = Linker::new(&ts, &fake);

    let without_session = linker.ask(&org_id, None, PASSWORD).await;
    assert_eq!(without_session.status(), StatusCode::UNAUTHORIZED);
    let wrong = linker
        .ask(&org_id, Some(&ts.client.token), "not the password")
        .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    assert!(cookies_set(&wrong).is_empty(), "nothing is started");
    let problem: serde_json::Value = wrong.json().await.expect("a problem");
    assert_eq!(problem["reason"], "invalid_credentials", "{problem}");
    assert_eq!(
        problem["errors"][0]["pointer"], "/linkPassword",
        "{problem}"
    );

    // What was meant to link does not sign in instead: a password that is no text,
    // or a member the request does not have, is refused, and so is a bearer token,
    // which is not what the browser comes back with.
    for (body, at) in [
        (json!({ "linkPassword": null }), "/linkPassword"),
        (json!({ "linkPassword": 7 }), "/linkPassword"),
        (json!({ "linkpassword": PASSWORD }), "/linkpassword"),
    ] {
        let malformed = ts
            .client
            .post(&format!("/auth/oidc/start?org={org_id}"), &body)
            .await;
        malformed.check_bad_request();
        assert_eq!(malformed.refused_at(), at, "{body}");
    }
    ts.client
        .post(
            &format!("/auth/oidc/start?org={org_id}"),
            &json!({ "linkPassword": PASSWORD }),
        )
        .await
        .check_unauthorized();
    // No body, and one without members, start a sign-in, as they did.
    ts.anonymous()
        .post(&format!("/auth/oidc/start?org={org_id}"), &json!({}))
        .await
        .check_ok();

    // An account made by the provider has no password to confirm with.
    let newcomer = through_provider(&ts, &fake, &org_id, "subject-of-a-newcomer").await;
    let no_password = linker.ask(&org_id, Some(&newcomer.token), PASSWORD).await;
    assert_eq!(no_password.status(), StatusCode::CONFLICT);
    let problem: serde_json::Value = no_password.json().await.expect("a problem");
    assert_eq!(problem["reason"], "password_not_set", "{problem}");
    assert_eq!(flows(&ts).await, 1, "but for the one that was to sign in");
    ts.teardown().await;
    fake.stop();
}

/// The account's password ties to it only the person the provider knows by the
/// account's own address, which the provider has checked: with the password alone,
/// nobody ties another person of the provider to the account, who would then sign in to
/// it, past an organization that takes no password, without it. Nothing is linked
/// otherwise, and the provider signs in to no account by it.
#[tokio::test]
async fn only_the_person_of_the_accounts_own_address_is_linked() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let user1: User = ts.client.me().await;
    let linker = Linker::new(&ts, &fake);
    let unchecked = Person {
        email_verified: Some(false),
        ..Person::new("subject-of-user1", "user1@sample.com")
    };
    let unmentioned = Person {
        email_verified: None,
        ..Person::new("subject-of-user1", "user1@sample.com")
    };
    let without = Person {
        email: None,
        ..Person::new("subject-of-user1", "user1@sample.com")
    };
    for (person, refusal) in [
        (
            Person::new("subject-of-another", "another@elsewhere.example"),
            "oidc-email-mismatch",
        ),
        // The address of another account is no more this account's.
        (
            Person::new("subject-of-another", "user2@sample.com"),
            "oidc-email-mismatch",
        ),
        (unchecked, "oidc-email-unverified"),
        (unmentioned, "oidc-email-unverified"),
        (without, "oidc-email-missing"),
    ] {
        fake.sign_in_as(person);
        assert_eq!(
            linker.link(&org_id, &ts.client.token).await,
            Ended::Refused(refusal.to_owned())
        );
        assert_eq!(identities(&ts.client).await, [], "{refusal}");
    }
    assert_eq!(flows(&ts).await, 0, "each sign-in is taken");

    // The same address in other letters is the account's.
    fake.sign_in_as(Person::new("subject-of-user1", "User1@Sample.com"));
    assert_eq!(linker.link(&org_id, &ts.client.token).await, Ended::Linked);
    let token = Browser::new(&ts, &fake)
        .sign_in(&org_id)
        .await
        .token()
        .to_owned();
    assert_eq!(ts.client_with_token(&token).me().await.id, user1.id);
    ts.teardown().await;
    fake.stop();
}

/// Who the provider vouches for is another account's: nothing is linked, and the two
/// accounts stay two.
#[tokio::test]
async fn an_identity_of_another_account_is_not_linked() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let theirs = through_provider(&ts, &fake, &org_id, "subject-of-somebody").await;
    let them: User = theirs.me().await;

    // The provider still vouches for that person when user1 links, now by user1's
    // address: whose the identity is decides, not what the provider says of it today.
    fake.sign_in_as(Person::new("subject-of-somebody", "user1@sample.com"));
    let linker = Linker::new(&ts, &fake);
    assert_eq!(
        linker.link(&org_id, &ts.client.token).await,
        Ended::Refused("oidc-identity-taken".to_owned())
    );
    assert_eq!(identities(&ts.client).await, []);
    assert_eq!(identities(&theirs).await.len(), 1, "theirs as before");
    let again = through_provider(&ts, &fake, &org_id, "subject-of-somebody").await;
    assert_eq!(again.me().await.id, them.id, "and still signs in to them");
    ts.teardown().await;
    fake.stop();
}

/// A link ends only in the browser that started it, with the session it was started
/// from, while that session is good: brought back to another session, without one,
/// without the cookie of the sign-in, or after the session ended, it ties nothing to
/// anybody.
#[tokio::test]
async fn a_link_ends_only_in_the_browser_and_the_session_that_started_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    fake.sign_in_as(Person::new("subject-of-user1", "user1@sample.com"));
    let linker = Linker::new(&ts, &fake);
    let mine = &ts.client.token;
    let invalid = Ended::Refused("oidc-state-invalid".to_owned());

    // To the session of somebody else, who must not get user1's sign-in tied to them.
    let returning = linker.go_to_provider(&org_id, mine).await;
    let ended = linker
        .come_back(&returning, Some(&returning.flow), Some(&ts.client2.token))
        .await;
    assert_eq!(ended, invalid, "another session");
    // Without a session.
    let returning = linker.go_to_provider(&org_id, mine).await;
    let ended = linker
        .come_back(&returning, Some(&returning.flow), None)
        .await;
    assert_eq!(ended, invalid, "no session");
    // In another browser, which has the session's cookie but not the sign-in's.
    let returning = linker.go_to_provider(&org_id, mine).await;
    let ended = linker.come_back(&returning, None, Some(mine)).await;
    assert_eq!(ended, invalid, "not the browser that started it");
    // After the session it was started from ended.
    let ending = ts.login(USER1_USERNAME, PASSWORD).await;
    let returning = linker.go_to_provider(&org_id, &ending.token).await;
    ending.post_empty("/logout").await.check_no_content();
    let ended = linker
        .come_back(&returning, Some(&returning.flow), Some(&ending.token))
        .await;
    assert_eq!(ended, invalid, "the session ended");

    // After an administrator set the account's password, which ends its sessions.
    let reset = ts.login(USER1_USERNAME, PASSWORD).await;
    let returning = linker.go_to_provider(&org_id, &reset.token).await;
    ts.app
        .reset_user_password(USER1_USERNAME, PASSWORD)
        .await
        .expect("the password is set");
    let ended = linker
        .come_back(&returning, Some(&returning.flow), Some(&reset.token))
        .await;
    assert_eq!(ended, invalid, "the sessions were ended");
    // The sessions of the test's clients went with it.
    let mine = &ts.login(USER1_USERNAME, PASSWORD).await.token;
    // After the provider was switched off while the person was at it.
    let returning = linker.go_to_provider(&org_id, mine).await;
    let admin = ts.client_with_token(mine);
    let settings = |enabled: bool| {
        json!({
            "issuer": fake.issuer, "clientId": crate::fake_oidc::CLIENT_ID, "enabled": enabled,
        })
    };
    admin
        .put(&format!("/org/{org_id}/oidc"), &settings(false))
        .await
        .check_ok();
    let ended = linker
        .come_back(&returning, Some(&returning.flow), Some(mine))
        .await;
    assert_eq!(ended, Ended::Refused("oidc-unavailable".to_owned()));
    admin
        .put(&format!("/org/{org_id}/oidc"), &settings(true))
        .await
        .check_ok();

    assert_eq!(identities(&admin).await, []);
    assert_eq!(identities(&ts.client2).await, []);
    // And as it is meant, it links.
    assert_eq!(linker.link(&org_id, mine).await, Ended::Linked);
    assert_eq!(identities(&admin).await.len(), 1);
    ts.teardown().await;
    fake.stop();
}

/// The session a link was started from changes its password meanwhile: it goes on
/// under a new token, which the browser has, and the link ends well with that one and
/// not with the token before; the account's other sessions end, and their links with
/// them.
#[tokio::test]
async fn a_link_follows_its_session_through_a_change_of_the_password() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    fake.sign_in_as(Person::new("subject-of-user2", "user2@sample.com"));
    let linker = Linker::new(&ts, &fake);
    let invalid = Ended::Refused("oidc-state-invalid".to_owned());
    let other = ts.login(crate::common::USER2_USERNAME, PASSWORD).await;

    let of_the_other = linker.go_to_provider(&org_id, &other.token).await;
    let returning = linker.go_to_provider(&org_id, &ts.client2.token).await;
    let before = linker.go_to_provider(&org_id, &ts.client2.token).await;
    let changed = ts
        .client2
        .post(
            "/users/me/password",
            &json!({ "oldPassword": PASSWORD, "newPassword": "another password" }),
        )
        .await;
    changed.check_no_content();
    let renewed = changed.session_cookie().expect("the session's new token");

    let ended = linker
        .come_back(&of_the_other, Some(&of_the_other.flow), Some(&other.token))
        .await;
    assert_eq!(ended, invalid, "the account's other session ended");
    let ended = linker
        .come_back(&before, Some(&before.flow), Some(&ts.client2.token))
        .await;
    assert_eq!(ended, invalid, "the token before is no session's");
    let ended = linker
        .come_back(&returning, Some(&returning.flow), Some(&renewed))
        .await;
    assert_eq!(
        ended,
        Ended::Linked,
        "the session went on, and its link with it"
    );
    assert_eq!(identities(&ts.client_with_token(&renewed)).await.len(), 1);
    ts.teardown().await;
    fake.stop();
}

/// A wrong password of a link or of a removal is counted with the failed logins to the
/// account: who holds a session does not get to guess at the password through either.
#[tokio::test]
async fn the_password_is_not_guessed_at_through_linking_or_removing() {
    let fake = FakeProvider::start().await;
    let ts = crate::common::Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(crate::oidc::with_key_and_private_addresses)
        .configure(|config| {
            config.login_pair_free_failures = 2;
            config.login_pair_base_delay_seconds = 30;
        })
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let linker = Linker::new(&ts, &fake);
    fake.sign_in_as(Person::new("subject-of-user1", "user1@sample.com"));
    assert_eq!(linker.link(&org_id, &ts.client.token).await, Ended::Linked);
    let mine = identities(&ts.client).await.remove(0);

    let wrong = linker
        .ask(&org_id, Some(&ts.client.token), "a wrong guess")
        .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    for _ in 0..2 {
        unlink(&ts.client, &mine, "a wrong guess")
            .await
            .check_bad_request();
    }
    // One too many, by either way, and for the right password and a login too.
    let limited = linker
        .ask(&org_id, Some(&ts.client.token), "a wrong guess")
        .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    unlink(&ts.client, &mine, PASSWORD)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    let mut login = ts.anonymous();
    login
        .login(USER1_USERNAME, PASSWORD)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(identities(&ts.client).await, [mine], "nothing was removed");
    ts.teardown().await;
    fake.stop();
}

/// A link ties an identity and does nothing else: through the provider of an
/// organization the account is no member of, it joins nobody to that organization,
/// though a sign-in through that provider would.
#[tokio::test]
async fn a_link_joins_no_organization() {
    use fvoci_store::DbType;
    use fvoci_store::testing::db_type_of_the_tests;
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let crate::sso::Two { ts, theirs, .. } = crate::sso::two_organizations(&fake).await;
    let user2: User = ts.client2.me().await;
    let their_store = ts.app.store().for_org(&theirs).expect("an identifier");
    let is_member = async || {
        their_store
            .is_organization_member(&user2.id)
            .await
            .expect("read")
    };
    assert!(!is_member().await, "user2 is of ours alone");

    fake.sign_in_as(Person::new("subject-of-user2", "user2@sample.com"));
    let linker = Linker::new(&ts, &fake);
    assert_eq!(linker.link(&theirs, &ts.client2.token).await, Ended::Linked);
    assert_eq!(identities(&ts.client2).await.len(), 1, "tied");
    assert!(!is_member().await, "and joined to nothing");
    // The sign-in through their provider is what joins, where they let newcomers in.
    let through = through_provider(&ts, &fake, &theirs, "subject-of-user2").await;
    assert_eq!(through.me().await.id, user2.id, "the same account");
    assert!(is_member().await, "joined by signing in");
    ts.teardown().await;
    fake.stop();
}

/// An identity is removed with the account's password; one that is not the account's
/// is not there; and an account without a password keeps the last one, which is all
/// that lets its owner in.
#[tokio::test]
async fn an_identity_is_removed_with_the_password_and_the_last_way_in_stays() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    fake.sign_in_as(Person::new("subject-of-user1", "user1@sample.com"));
    let linker = Linker::new(&ts, &fake);
    assert_eq!(linker.link(&org_id, &ts.client.token).await, Ended::Linked);
    let mine = identities(&ts.client).await.remove(0);

    let wrong = unlink(&ts.client, &mine, "not the password").await;
    wrong.check_bad_request();
    assert_eq!(wrong.error_reason(), "invalid_credentials");
    assert_eq!(wrong.refused_at(), "/password");
    let incomplete = ts
        .client
        .delete_json("/users/me/identities", &json!({ "issuer": mine.issuer }))
        .await;
    incomplete.check_bad_request();
    assert_eq!(incomplete.refused_places(), ["/subject"]);
    // Not user2's: as one that is nobody's.
    let not_theirs = unlink(&ts.client2, &mine, PASSWORD).await;
    let nobodys = unlink(
        &ts.client2,
        &LinkedIdentity {
            subject: "nobody".to_owned(),
            ..mine.clone()
        },
        PASSWORD,
    )
    .await;
    not_theirs.check_not_found();
    nobodys.check_not_found();
    assert_eq!(not_theirs.error_message(), nobodys.error_message());
    assert_eq!(
        identities(&ts.client).await,
        std::slice::from_ref(&mine),
        "still there"
    );

    // An account without a password: its only identity stays, and with a second one
    // it has no password to confirm removing either with.
    let newcomer = through_provider(&ts, &fake, &org_id, "subject-of-a-newcomer").await;
    let only = identities(&newcomer).await.remove(0);
    // Whatever is sent for the password it does not have.
    for password in [PASSWORD, ""] {
        let last = unlink(&newcomer, &only, password).await;
        last.check_status(StatusCode::CONFLICT);
        assert_eq!(last.error_reason(), "last_sign_in");
    }
    ts.app
        .store()
        .add_identity(
            &newcomer.me().await.id,
            Identity {
                issuer: "https://another.example.com",
                subject: "second",
            },
            "",
        )
        .await
        .expect("a second identity");
    let unconfirmed = unlink(&newcomer, &only, PASSWORD).await;
    unconfirmed.check_status(StatusCode::CONFLICT);
    assert_eq!(unconfirmed.error_reason(), "password_not_set");
    assert_eq!(identities(&newcomer).await.len(), 2, "both stay");

    unlink(&ts.client, &mine, PASSWORD).await.check_no_content();
    assert_eq!(identities(&ts.client).await, []);
    fake.sign_in_as(Person::new("subject-of-user1", "user1@sample.com"));
    assert_eq!(
        Browser::new(&ts, &fake).sign_in(&org_id).await,
        Outcome::Refused("oidc-email-taken".to_owned()),
        "the provider signs in to the account no more"
    );
    ts.teardown().await;
    fake.stop();
}

/// The way in to an organization that takes no password, for a member who has one:
/// the password session opens nothing of it and still links, and the provider's
/// session then opens it.
#[tokio::test]
async fn a_member_shut_out_links_and_gets_in_through_the_provider() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    let user2: User = ts.client2.me().await;
    ts.client2
        .get(&boards)
        .await
        .check_status(StatusCode::UNAUTHORIZED);

    fake.sign_in_as(Person::new("subject-of-user2", "user2@sample.com"));
    let linker = Linker::new(&ts, &fake);
    assert_eq!(linker.link(&org_id, &ts.client2.token).await, Ended::Linked);
    let token = Browser::new(&ts, &fake)
        .sign_in(&org_id)
        .await
        .token()
        .to_owned();
    let through = ts.client_with_token(&token);
    assert_eq!(through.me().await.id, user2.id, "the same account");
    through.get(&boards).await.check_ok();
    ts.teardown().await;
    fake.stop();
}
