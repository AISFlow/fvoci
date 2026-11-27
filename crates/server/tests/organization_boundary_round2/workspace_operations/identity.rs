//! The existing provider contract also works when signup created a later organization.

use base64::Engine as _;
use reqwest::{Client as HttpClient, Method, StatusCode, header};
use serde_json::json;

use fvoci_model::OrgKind;
use fvoci_server::auth::sealed::SecretKey;
use fvoci_server::config::Configuration;

use super::super::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Person};
use super::{Case, Client, TestServer};

pub(super) fn configure(config: &mut Configuration) {
    config.secret_key = Some(
        SecretKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([42_u8; 32]))
            .expect("test key"),
    );
    config.oidc_allow_private_addresses = true;
}

pub(super) async fn exercise(case: &mut Case, server: &TestServer, fake: &FakeProvider) {
    fake.sign_in_as(Person::new(
        &format!("{}-subject", case.label),
        &format!("{}-oidc@example.test", case.label),
    ));
    let configured = case.run("putOidcProvider", &[], json!({"issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET, "autoJoin": true})).await;
    let provider = case
        .present("getOidcProvider", &[], &fake.issuer)
        .await
        .value();
    assert_eq!(provider, configured.value(), "provider settings round trip");
    assert_eq!(
        provider["redirectUri"],
        format!(
            "{}/api/v1/auth/oidc/callback/{}",
            server.app.config().server_url.trim_end_matches('/'),
            case.org
        ),
        "provider redirect names this organization"
    );
    sign_in(case, server, fake).await;
    case.run("deleteOidcProvider", &[], json!({})).await;
}

async fn sign_in(case: &mut Case, server: &TestServer, fake: &FakeProvider) {
    let request = super::with_query(case.request("startOidcLogin", &[]), "org", &case.org);
    let started = case.send("startOidcLogin", request).await;
    let cookie = started
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            value
                .split(';')
                .next()
                .filter(|value| value.starts_with("fvoci_oidc="))
        })
        .expect("browser-bound login cookie");
    let http = HttpClient::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tls_certs_merge([
            reqwest::Certificate::from_der(&fake.certificate_der).expect("test certificate")
        ])
        .build()
        .expect("browser client");
    let started = started.value();
    let authorization = started["authorizationUrl"]
        .as_str()
        .expect("authorization URL");
    assert!(
        authorization.starts_with(&fake.issuer),
        "login goes to this provider"
    );
    let authorized = http
        .get(authorization)
        .send()
        .await
        .expect("provider answers");
    assert_eq!(
        authorized.status(),
        StatusCode::FOUND,
        "provider returns the browser"
    );
    let callback = authorized.headers()[header::LOCATION]
        .to_str()
        .expect("callback");
    let path = callback
        .strip_prefix(server.app.config().server_url.trim_end_matches('/'))
        .expect("callback to this server");
    assert!(
        path.starts_with(&format!("/api/v1/auth/oidc/callback/{}?", case.org)),
        "callback identifies this organization"
    );
    let completed = Client::send(
        http.get(format!("{}{path}", server.base_url))
            .header(header::COOKIE, cookie),
    )
    .await;
    assert_eq!(
        completed.status,
        StatusCode::SEE_OTHER,
        "callback completes sign-in"
    );
    assert_eq!(
        completed.headers[header::LOCATION],
        format!("{}/", server.app.config().server_url.trim_end_matches('/')),
        "successful callback opens the app"
    );
    let session = completed
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.split(';').next()?.strip_prefix("fvoci_session="))
        .expect("OIDC completed with a session");
    let newcomer = Client::new(&server.base_url, session);
    let workspaces = newcomer.get("/users/me/workspaces").await.value();
    let kinds: Vec<Option<&str>> = workspaces
        .as_array()
        .expect("workspaces")
        .iter()
        .map(|workspace| workspace["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [OrgKind::Shared, OrgKind::Personal].map(|kind| Some(kind.as_str())),
        "OIDC newcomer joins one workspace, and has a space of their own"
    );
    assert_eq!(
        workspaces[0]["id"], case.workspace,
        "OIDC follows its organization"
    );
    case.record("finishOidcLogin", &completed);
    linked(case, &newcomer, fake).await;
    pending(case, &newcomer, fake).await;
    held(case).await;
}

/// The two ends of a sign-in held while an organization moves to another provider:
/// no sign-in of this organization is held for the browser that asks, and each end
/// answers so, the same in both organizations, and makes no session.
async fn held(case: &mut Case) {
    for operation in ["startOidcCarryOver", "finishHeldOidcLogin"] {
        let request = super::with_query(case.request(operation, &[]), "org", &case.org);
        let refused = Client::send(request).await;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text());
        assert_eq!(
            refused.value()["reason"],
            "oidc_carry_over_refused",
            "{}",
            refused.text()
        );
        let sets_a_session = refused
            .headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|cookie| cookie.to_str().ok())
            .any(|cookie| cookie.contains("session=") && !cookie.contains("Max-Age=0"));
        assert!(
            !sets_a_session,
            "{operation} sets no session when it is refused"
        );
        case.record(operation, &refused);
    }
}

/// The provider's pending settings, by an administrator whose session was made through
/// it: the newcomer, made one, requires single sign-on, saves another secret, which
/// waits aside, starts a trial of it and throws it away, ends the keeping of a former
/// provider, and takes the requirement off again; then the founder makes them a
/// member again, so that what follows is as before.
async fn pending(case: &mut Case, newcomer: &Client, fake: &FakeProvider) {
    let id = newcomer.me().await.id;
    case.run("putOrgRole", &[("userId", &id)], json!({"role": "admin"}))
        .await;
    let path = format!("/org/{}/oidc", case.org);
    let settings = |extra: serde_json::Value| {
        let mut body = json!({"issuer": fake.issuer, "clientId": CLIENT_ID});
        body.as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        body
    };
    for (body, aside) in [
        (settings(json!({"ssoRequired": true})), false),
        (settings(json!({"clientSecret": "the next secret"})), true),
    ] {
        let saved = Client::send(newcomer.request(Method::PUT, &path).json(&body)).await;
        assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
        assert_eq!(
            !saved.value()["pending"].is_null(),
            aside,
            "{}",
            saved.text()
        );
    }
    let trial = format!("{path}/pending/sign-in");
    let started = case
        .send("startOidcTrial", newcomer.request(Method::POST, &trial))
        .await
        .value();
    assert!(
        started["authorizationUrl"]
            .as_str()
            .is_some_and(|url| url.starts_with(&fake.issuer)),
        "a trial goes to this provider: {started}"
    );
    let discard = format!("{path}/pending");
    case.send(
        "deleteOidcPending",
        newcomer.request(Method::DELETE, &discard),
    )
    .await;
    // The organization moved from no other provider: there is no former one to end,
    // and ending none answers the same.
    let former = format!("{path}/former");
    case.send(
        "deleteOidcFormerProvider",
        newcomer
            .request(Method::DELETE, &former)
            .json(&json!({"createdAt": 1})),
    )
    .await;
    let released = Client::send(
        newcomer
            .request(Method::PUT, &path)
            .json(&settings(json!({"ssoRequired": false}))),
    )
    .await;
    assert_eq!(released.status, StatusCode::OK, "{}", released.text());
    case.run("putOrgRole", &[("userId", &id)], json!({"role": "member"}))
        .await;
}

/// The account the provider made in this organization lists the identity it was made
/// with, and keeps it: it has no password, so its last way in is not removed.
async fn linked(case: &mut Case, newcomer: &Client, fake: &FakeProvider) {
    let listed = case
        .send(
            "getMyIdentities",
            newcomer.request(Method::GET, "/users/me/identities"),
        )
        .await
        .value();
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(1),
        "the provider's account has its identity: {listed}"
    );
    assert_eq!(listed[0]["issuer"], fake.issuer.as_str(), "{listed}");
    let kept = Client::send(
        newcomer
            .request(Method::DELETE, "/users/me/identities")
            .json(&json!({
                "issuer": listed[0]["issuer"],
                "subject": listed[0]["subject"],
                "password": "",
            })),
    )
    .await;
    assert_eq!(kept.status, StatusCode::CONFLICT, "{}", kept.text());
    assert_eq!(kept.error_reason(), "last_sign_in", "its only way in stays");
    case.record("unlinkIdentity", &kept);
}
