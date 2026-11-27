//! Signing in through a provider when things happen meanwhile: the provider changed
//! while the person is at it, a sign-in that ended or comes back twice, a start that
//! another site's page or a forged host would make.

use std::time::Duration;

use base64::Engine as _;
use fvoci_model::{OidcProvider, OidcStart, UserWorkspace};
use fvoci_server::app::{Callback, FinishedLogin, LoginFailure, Returned};
use fvoci_server::auth::sealed::SecretKey;
use fvoci_server::auth::secret::token_digest;
use fvoci_server::config::Configuration;
use fvoci_server::error::Reason;
use serde_json::json;

use crate::common::{Setup, TestServer};
use crate::fake_oidc::{self, AnswerIssuer, CLIENT_ID, CLIENT_SECRET, FakeProvider};

fn configure(config: &mut Configuration) {
    config.secret_key = Some(
        SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([42_u8; 32]))
            .expect("a test key"),
    );
    config.oidc_allow_private_addresses = true;
}

async fn server_for(fake: &FakeProvider) -> (TestServer, String) {
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(configure)
        .start()
        .await
        .init_basic()
        .await;
    let workspaces: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    let org_id = workspaces
        .first()
        .expect("the administrator's workspace")
        .org_id
        .clone();
    ts.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET }),
        )
        .await
        .check_ok();
    (ts, org_id)
}

/// A real flow and a code issued for its nonce, redirect address and PKCE challenge.
async fn callback_for(ts: &TestServer, fake: &FakeProvider, org_id: &str) -> Callback {
    let started = ts
        .app
        .start_oidc_login(org_id, None)
        .await
        .expect("the sign-in starts");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .tls_certs_merge([
            reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
        ])
        .build()
        .expect("a client");
    let authorized = http
        .get(&started.authorization_url)
        .send()
        .await
        .expect("the provider answers");
    assert_eq!(
        authorized.status(),
        reqwest::StatusCode::FOUND,
        "the provider sends the browser back"
    );
    let location = authorized
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("the callback address");
    let url = reqwest::Url::parse(location.to_str().expect("an address")).expect("a callback URL");
    let get = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .expect("the callback parameter")
            .1
            .into_owned()
    };
    Callback {
        state: get("state"),
        code: get("code"),
        browser_secret: started.browser_secret,
        error: None,
        org_id: url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .expect("the organization's own address")
            .to_owned(),
        issuer: None,
        session_token: None,
        held_secret: None,
        accept_language: None,
    }
}

#[derive(Debug, Clone, Copy)]
enum Change {
    Disable,
    Delete,
    CloseJoining,
    /// Given another client: another provider, under a new identifier.
    Replace,
}

/// Who comes back while the provider changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    /// Somebody with no account yet.
    Newcomer,
    /// The account of the identity, a member of the organization.
    Member,
    /// The account of the identity, which left the organization.
    FormerMember,
}

/// Makes the account of the fake provider's person by a whole sign-in, and takes it
/// out of the organization again if it is to be no member.
async fn known_account(ts: &TestServer, fake: &FakeProvider, org_id: &str, who: Who) {
    let callback = callback_for(ts, fake, org_id).await;
    let Returned {
        result: signed_in, ..
    } = ts.app.finish_oidc_login(&callback, "the first time").await;
    signed_in.expect("the account is made");
    if who == Who::FormerMember {
        for table in ["workspace_members", "organization_members"] {
            ts.app
                .store()
                .system()
                .execute_unchecked(&format!(
                    "DELETE FROM {table} WHERE user_id IN (SELECT user_id FROM user_identities)"
                ))
                .await
                .expect("the account leaves");
        }
    }
}

/// Makes the administrator's change of the provider, through the API.
async fn change_provider(ts: &TestServer, fake: &FakeProvider, org_id: &str, change: Change) {
    let path = format!("/org/{org_id}/oidc");
    match change {
        Change::Delete => {
            let deleted = ts.client.delete(&path).await;
            assert_eq!(
                deleted.status,
                reqwest::StatusCode::NO_CONTENT,
                "the provider is deleted"
            );
            ts.client.get(&path).await.check_not_found();
        }
        Change::Replace => {
            let replaced: OidcProvider = ts
                .client
                .put(
                    &path,
                    &json!({
                        "issuer": fake.issuer,
                        "clientId": "another-client",
                        "clientSecret": "another secret",
                    }),
                )
                .await
                .ok_json();
            assert_eq!(
                replaced.client_id, "another-client",
                "the provider was replaced"
            );
        }
        Change::Disable | Change::CloseJoining => {
            let changed: OidcProvider = ts
                .client
                .put(
                    &path,
                    &json!({
                        "issuer": fake.issuer,
                        "clientId": CLIENT_ID,
                        "enabled": !matches!(change, Change::Disable),
                        "autoJoin": !matches!(change, Change::CloseJoining),
                    }),
                )
                .await
                .ok_json();
            assert_eq!(
                changed.enabled,
                !matches!(change, Change::Disable),
                "the enabled flag was updated"
            );
            assert_eq!(
                changed.auto_join,
                !matches!(change, Change::CloseJoining),
                "the auto-join flag was updated"
            );
        }
    }
}

/// Pause only this callback after its token request reaches the real fake provider.
/// The administrator's HTTP request then completes before verification and joining
/// continue. No production code, clock or provider response is replaced.
async fn changed_provider_refuses_pending_callback(change: Change, who: Who) {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    if who != Who::Newcomer {
        known_account(&ts, &fake, &org_id, who).await;
    }
    let callback = callback_for(&ts, &fake, &org_id).await;
    let exchanged_before = fake.token_requests();
    let users_before = ts
        .app
        .store()
        .count_rows("users")
        .await
        .expect("users counted");
    let members_before = ts
        .app
        .store()
        .count_rows("organization_members")
        .await
        .expect("members counted");
    let Returned {
        result: outcome, ..
    } = {
        let finishing = ts.app.finish_oidc_login(&callback, "a returning browser");
        tokio::pin!(finishing);
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                biased;
                () = async {
                    while fake.token_requests() == exchanged_before {
                        tokio::task::yield_now().await;
                    }
                } => {}
                result = &mut finishing => panic!("the callback completed before the checkpoint: {result:?}"),
            }
        })
        .await
        .expect("the callback reaches the token exchange");
        change_provider(&ts, &fake, &org_id, change).await;
        tokio::time::timeout(Duration::from_secs(10), finishing)
            .await
            .expect("the callback finishes")
    };
    let live_session = match &outcome {
        Ok(FinishedLogin::SignedIn { token } | FinishedLogin::Applied { token }) => ts
            .app
            .store()
            .get_session(&token_digest(token), 0, 0)
            .await
            .is_ok(),
        Ok(FinishedLogin::Linked | FinishedLogin::Held { .. }) | Err(_) => false,
    };
    let users_after = ts
        .app
        .store()
        .count_rows("users")
        .await
        .expect("users counted");
    let members_after = ts
        .app
        .store()
        .count_rows("organization_members")
        .await
        .expect("members counted");
    ts.teardown().await;
    fake.stop();
    let expected = match change {
        Change::Disable | Change::Delete | Change::Replace => Reason::OidcUnavailable,
        Change::CloseJoining => Reason::OidcJoinClosed,
    };
    assert_eq!(
        outcome.err(),
        Some(LoginFailure::Reason(expected)),
        "{change:?} completed before the callback finished, but users changed from {users_before} to {users_after}, live session: {live_session}",
    );
    assert_eq!(users_after, users_before, "no newcomer after {change:?}");
    assert_eq!(
        members_after, members_before,
        "no new member after {change:?}"
    );
    assert!(!live_session, "no session after {change:?}");
}

#[tokio::test]
async fn disabling_a_provider_during_token_exchange_prevents_a_new_session() {
    Box::pin(changed_provider_refuses_pending_callback(
        Change::Disable,
        Who::Newcomer,
    ))
    .await;
    Box::pin(changed_provider_refuses_pending_callback(
        Change::Disable,
        Who::Member,
    ))
    .await;
}

#[tokio::test]
async fn deleting_a_provider_during_token_exchange_prevents_a_new_session() {
    Box::pin(changed_provider_refuses_pending_callback(
        Change::Delete,
        Who::Newcomer,
    ))
    .await;
}

#[tokio::test]
async fn replacing_a_provider_during_token_exchange_prevents_a_new_session() {
    Box::pin(changed_provider_refuses_pending_callback(
        Change::Replace,
        Who::Newcomer,
    ))
    .await;
    Box::pin(changed_provider_refuses_pending_callback(
        Change::Replace,
        Who::Member,
    ))
    .await;
}

#[tokio::test]
async fn closing_auto_join_during_token_exchange_prevents_a_new_member() {
    Box::pin(changed_provider_refuses_pending_callback(
        Change::CloseJoining,
        Who::Newcomer,
    ))
    .await;
    Box::pin(changed_provider_refuses_pending_callback(
        Change::CloseJoining,
        Who::FormerMember,
    ))
    .await;
}

#[tokio::test]
async fn an_expired_flow_is_consumed_without_exchanging_its_code() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    let callback = callback_for(&ts, &fake, &org_id).await;
    ts.app
        .store()
        .for_org(&org_id)
        .expect("the organization")
        .execute_unchecked("UPDATE oidc_login_flows SET expires_at = 0")
        .await
        .expect("the flow expires");
    let Returned {
        result: outcome, ..
    } = ts
        .app
        .finish_oidc_login(&callback, "a returning browser")
        .await;
    let flow_count = ts
        .app
        .store()
        .for_org(&org_id)
        .expect("the organization")
        .count_rows("oidc_login_flows")
        .await
        .expect("flows counted");
    let token_requests = fake.token_requests();
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        outcome.err(),
        Some(LoginFailure::Reason(Reason::OidcStateInvalid)),
        "an expired flow is refused",
    );
    assert_eq!(flow_count, 0, "the expired flow is removed");
    assert_eq!(
        token_requests, 0,
        "no code is exchanged for an expired flow"
    );
}

#[tokio::test]
async fn concurrent_callbacks_consume_a_flow_once() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    let callback = callback_for(&ts, &fake, &org_id).await;
    let (Returned { result: first, .. }, Returned { result: second, .. }) = tokio::join!(
        ts.app.finish_oidc_login(&callback, "first return"),
        ts.app.finish_oidc_login(&callback, "second return"),
    );
    let accepted = usize::from(first.is_ok()) + usize::from(second.is_ok());
    let refusals: Vec<_> = [first.err(), second.err()].into_iter().flatten().collect();
    let token_requests = fake.token_requests();
    ts.teardown().await;
    fake.stop();
    assert_eq!(accepted, 1, "only one callback opens a session");
    assert_eq!(
        refusals,
        [LoginFailure::Reason(Reason::OidcStateInvalid)],
        "the other callback finds no flow"
    );
    assert_eq!(token_requests, 1, "only one code exchange");
}

#[tokio::test]
async fn a_start_with_a_forged_host_keeps_the_configured_callback_address() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("a client");
    let started = http
        .post(format!(
            "{}/api/v1/auth/oidc/start?org={org_id}",
            ts.base_url
        ))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Host", "attacker.example")
        .header("X-Forwarded-Host", "attacker.example")
        .header("X-Forwarded-Proto", "https")
        .send()
        .await
        .expect("the server answers");
    let status = started.status();
    let answer: OidcStart = started.json().await.expect("where the browser goes");
    let authorization = reqwest::Url::parse(&answer.authorization_url).expect("a URL");
    let redirect = authorization
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .expect("the callback address")
        .1
        .into_owned();
    let expected = format!(
        "{}/api/v1/auth/oidc/callback/{org_id}",
        ts.app.config().server_url.trim_end_matches('/')
    );
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "the start answers where the browser goes"
    );
    assert_eq!(
        redirect, expected,
        "request headers cannot choose the callback host"
    );
}
/// Fetch Metadata is omitted for ordinary HTTP origins. The same absent-header
/// request can come from a real browser following a link on another site's page;
/// it is not evidence that the caller is a non-browser client. The provider used
/// here signs every arriving browser in as an account chosen by its operator. So no
/// start may be one that another site's page can make a browser send: not an address
/// it is sent to, not a form posted to it, whatever headers of Fetch Metadata come or
/// do not come. Only a request of the server's own page starts a sign-in.
#[tokio::test]
async fn a_cross_site_navigation_without_fetch_metadata_cannot_sign_in() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    fake.sign_in_as(fake_oidc::Person::new("attacker", "attacker@example.com"));
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("a browser client");
    let start = format!("{}/api/v1/auth/oidc/start?org={org_id}", ts.base_url);
    let flows_before = flows(&ts, &org_id).await;
    let sends = [
        // Followed from a link or sent there by a script of another page.
        http.get(&start)
            .header("Referer", "http://attacker.example/"),
        // A form of another page, posted: no header of the server's scripts.
        http.post(&start)
            .header("Origin", "http://attacker.example")
            .header("Content-Type", "application/x-www-form-urlencoded"),
        // Even with that header (which a page of another site cannot add), a foreign origin.
        http.post(&start)
            .header("Origin", "http://attacker.example")
            .header("X-Requested-With", "XMLHttpRequest"),
    ];
    let mut answers = Vec::new();
    for send in sends {
        let answer = send.send().await.expect("the server answers");
        let flow_cookie = answer
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|cookie| cookie.starts_with("fvoci_oidc="));
        answers.push((answer.status(), flow_cookie));
    }
    let flows_after = flows(&ts, &org_id).await;
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        answers,
        [
            (reqwest::StatusCode::METHOD_NOT_ALLOWED, false),
            (reqwest::StatusCode::FORBIDDEN, false),
            (reqwest::StatusCode::FORBIDDEN, false),
        ],
        "no sign-in starts from another site's page"
    );
    assert_eq!(flows_after, flows_before, "no sign-in was stored");
}

/// The sign-ins under way of an organization.
async fn flows(ts: &TestServer, org_id: &str) -> i64 {
    ts.app
        .store()
        .for_org(org_id)
        .expect("the organization")
        .count_rows("oidc_login_flows")
        .await
        .expect("flows counted")
}

/// An answer of one provider handed to another is no sign-in: a return to the address of
/// another organization than the one the sign-in was started for, or that says another
/// issuer answered (RFC 9207), is refused before its code is exchanged anywhere. So an
/// administrator of one organization cannot route another organization's code through
/// a provider of theirs.
#[tokio::test]
async fn an_answer_of_one_provider_is_not_taken_for_another() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    let mut outcomes = Vec::new();
    for change in [
        |callback: &mut Callback| callback.org_id = fvoci_model::new_id(),
        |callback: &mut Callback| callback.issuer = Some("https://elsewhere.example".to_owned()),
    ] {
        let mut callback = callback_for(&ts, &fake, &org_id).await;
        change(&mut callback);
        let exchanged_before = fake.token_requests();
        let Returned {
            result: outcome, ..
        } = ts
            .app
            .finish_oidc_login(&callback, "a returning browser")
            .await;
        outcomes.push((outcome.err(), fake.token_requests() - exchanged_before));
    }
    // The issuer the provider is, said: it goes through.
    let mut callback = callback_for(&ts, &fake, &org_id).await;
    callback.issuer = Some(fake.issuer.clone());
    let Returned { result: said, .. } = ts
        .app
        .finish_oidc_login(&callback, "a returning browser")
        .await;
    ts.teardown().await;
    fake.stop();
    let refused = Some(LoginFailure::Reason(Reason::OidcStateInvalid));
    assert_eq!(
        outcomes,
        [(refused, 0), (refused, 0)],
        "refused, and no code exchanged"
    );
    assert!(said.is_ok(), "{said:?}");
}

/// A return without the cookie of the browser that started the sign-in takes nothing:
/// one who learned the `state` cannot end the sign-in of the browser that has it.
#[tokio::test]
async fn another_browser_does_not_end_a_sign_in() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    let callback = callback_for(&ts, &fake, &org_id).await;
    let mut other = callback.clone();
    other.browser_secret = fvoci_model::new_id();
    let Returned {
        org_id: other_org,
        result: refused,
        ..
    } = ts.app.finish_oidc_login(&other, "another browser").await;
    let Returned {
        result: finished, ..
    } = ts.app.finish_oidc_login(&callback, "the browser").await;
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        refused.err(),
        Some(LoginFailure::Reason(Reason::OidcStateInvalid))
    );
    assert_eq!(
        other_org.as_deref(),
        Some(org_id.as_str()),
        "back to the organization's login page"
    );
    assert!(
        finished.is_ok(),
        "the browser that started it ends it: {finished:?}"
    );
}

/// A provider that says it names who answered (RFC 9207) must: a return without `iss`
/// from it is refused before its code is exchanged; with its own issuer it goes through.
#[tokio::test]
async fn a_provider_that_promises_iss_must_send_it() {
    let fake = FakeProvider::start().await;
    let (ts, org_id) = server_for(&fake).await;
    fake.answer_issuer(AnswerIssuer::Promises);
    let callback = callback_for(&ts, &fake, &org_id).await;
    let exchanged_before = fake.token_requests();
    let Returned {
        result: missing, ..
    } = ts
        .app
        .finish_oidc_login(&callback, "a returning browser")
        .await;
    let exchanged = fake.token_requests() - exchanged_before;
    fake.answer_issuer(AnswerIssuer::Says);
    let mut callback = callback_for(&ts, &fake, &org_id).await;
    callback.issuer = Some(fake.issuer.clone());
    let Returned { result: said, .. } = ts
        .app
        .finish_oidc_login(&callback, "a returning browser")
        .await;
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        (missing.err(), exchanged),
        (Some(LoginFailure::Reason(Reason::OidcStateInvalid)), 0),
        "refused, and no code exchanged"
    );
    assert!(said.is_ok(), "{said:?}");
}
