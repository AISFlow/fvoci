//! Requests as a tunnel such as cloudflared brings them: from a proxy the server trusts,
//! with the person's address at the right of `X-Forwarded-For` and in
//! `CF-Connecting-IP`. The limits of sign-up, login and password reset count each person
//! by the address the proxy added, never by the proxy's own; the header the server reads
//! is `X-Forwarded-For` alone, and from no peer but a trusted proxy.

use fvoci_model::RegisterRequest;
use fvoci_server::config::Configuration;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, PASSWORD, Setup, TestServer};
use crate::fake_smtp::FakeSmtp;

/// The headers of one request: what `X-Forwarded-For` and `CF-Connecting-IP` say, if
/// they are sent.
#[derive(Debug, Clone, Copy, Default)]
struct Headers<'a> {
    forwarded_for: Option<&'a str>,
    connecting_ip: Option<&'a str>,
}

/// A person's request as the tunnel brings it.
fn of(address: &str) -> Headers<'_> {
    Headers {
        forwarded_for: Some(address),
        connecting_ip: Some(address),
    }
}

async fn post(th: &TestServer, path: &str, body: &Value, headers: Headers<'_>) -> ApiResponse {
    let mut request = th.anonymous().request(Method::POST, path).json(body);
    if let Some(forwarded_for) = headers.forwarded_for {
        request = request.header("X-Forwarded-For", forwarded_for);
    }
    if let Some(connecting_ip) = headers.connecting_ip {
        request = request.header("CF-Connecting-IP", connecting_ip);
    }
    Client::send(request).await
}

fn registration(username: &str) -> Value {
    serde_json::to_value(RegisterRequest {
        username: username.to_owned(),
        email: format!("{username}@sample.com"),
        password: PASSWORD.to_owned(),
        display_name: String::new(),
        organization_name: format!("{username}'s firm"),
        language: String::new(),
    })
    .expect("a registration is JSON")
}

fn login(username: &str) -> Value {
    json!({"username": username, "password": PASSWORD})
}

fn reset(username: &str) -> Value {
    json!({"email": format!("{username}@sample.com"), "language": "en"})
}

const REGISTER: &str = "/register";
const LOGIN: &str = "/login";
const RESET: &str = "/password-reset/request";

/// A server that sends mail (a reset needs it), with what `trust` sets: an address may
/// make two attempts a minute, and the proxies it trusts.
async fn server(smtp: &FakeSmtp, trust: fn(&mut Configuration)) -> TestServer {
    Setup::native()
        .smtp(smtp.settings())
        .configure(trust)
        .start()
        .await
}

/// Two people behind the same tunnel: one who has used up their attempts waits, the
/// other does not, and what the first writes into the headers does not change that.
#[tokio::test]
async fn behind_a_trusted_tunnel_each_person_has_their_own_limits() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = server(&smtp, |config| {
        config.login_address_attempts_per_minute = 2;
        config.trusted_proxies = vec!["127.0.0.1".to_owned(), "::1".to_owned()];
    })
    .await;
    let alice = "203.0.113.10";
    let bob = "198.51.100.20";

    post(&th, REGISTER, &registration("alice"), of(alice))
        .await
        .check_created();
    post(&th, LOGIN, &login("alice"), of(alice))
        .await
        .check_no_content();
    post(&th, LOGIN, &login("alice"), of(alice))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    // The second person's sign-up is not held up by the first one's count: it reaches
    // the server's own answer (it holds one organization, an SQLite database's), and a
    // login after it gets through.
    post(&th, REGISTER, &registration("bob"), of(bob))
        .await
        .check_status(StatusCode::FORBIDDEN);
    post(&th, LOGIN, &login("alice"), of(bob))
        .await
        .check_no_content();

    // What a person writes at the left of X-Forwarded-For stays there: the tunnel adds
    // the address it saw at the right. CF-Connecting-IP is not read at all. The address
    // claimed is one with attempts left, which a server believing it would let through.
    let claimed = "192.0.2.77";
    let forged = format!("{claimed}, {alice}");
    for headers in [
        Headers {
            forwarded_for: Some(&forged),
            connecting_ip: Some(claimed),
        },
        Headers {
            forwarded_for: Some(alice),
            connecting_ip: Some(claimed),
        },
    ] {
        post(&th, LOGIN, &login("alice"), headers)
            .await
            .check_status(StatusCode::TOO_MANY_REQUESTS);
    }
    post(&th, LOGIN, &login("alice"), of(claimed))
        .await
        .check_no_content();

    // Without X-Forwarded-For a request is the tunnel's own, and CF-Connecting-IP does
    // not stand in for it: once the tunnel's address has used its attempts, a request
    // naming a fresh address there waits too.
    for _ in 0..2 {
        post(&th, LOGIN, &login("alice"), Headers::default())
            .await
            .check_no_content();
    }
    let only_cloudflare = Headers {
        forwarded_for: None,
        connecting_ip: Some("192.0.2.88"),
    };
    post(&th, LOGIN, &login("alice"), only_cloudflare)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    // A reset has a count of its own, by the same address.
    for _ in 0..2 {
        post(&th, RESET, &reset("alice"), of(alice))
            .await
            .check_no_content();
    }
    post(&th, RESET, &reset("alice"), of(alice))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    post(&th, RESET, &reset("alice"), of(bob))
        .await
        .check_no_content();

    th.teardown().await;
    smtp.stop().await;
}

/// A server that trusts some proxy, but not the peer the requests come from: whatever
/// the headers say, every request is the peer's, and the peer's attempts run out.
#[tokio::test]
async fn from_a_peer_that_is_no_trusted_proxy_the_headers_are_not_believed() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = server(&smtp, |config| {
        config.login_address_attempts_per_minute = 2;
        config.trusted_proxies = vec!["10.0.0.0/8".to_owned()];
    })
    .await;

    post(&th, REGISTER, &registration("carol"), of("203.0.113.1"))
        .await
        .check_created();
    post(&th, LOGIN, &login("carol"), of("203.0.113.2"))
        .await
        .check_no_content();
    post(&th, LOGIN, &login("carol"), of("203.0.113.3"))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    for address in ["203.0.113.4", "203.0.113.5"] {
        post(&th, RESET, &reset("carol"), of(address))
            .await
            .check_no_content();
    }
    post(&th, RESET, &reset("carol"), of("203.0.113.6"))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    th.teardown().await;
    smtp.stop().await;
}

/// Somebody who signs up on their own, with no organization, is counted once against
/// their address, as one who starts an organization is: of an address's two attempts a
/// minute, two such sign-ups go ahead and the third waits.
#[tokio::test]
async fn a_sign_up_on_ones_own_is_counted_once_against_the_address() {
    // A server of several organizations, where one signs up on one's own, is PostgreSQL's.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let smtp = FakeSmtp::start_on(0).await;
    let th = server(&smtp, |config| {
        config.login_address_attempts_per_minute = 2;
        config.trusted_proxies = vec!["127.0.0.1".to_owned(), "::1".to_owned()];
        config.org_creation = fvoci_server::config::ORG_CREATION_OPEN.to_owned();
    })
    .await;
    // The server's first account starts its organization, from another address.
    post(&th, REGISTER, &registration("first"), of("198.51.100.30"))
        .await
        .check_created();
    let alone = |username: &str| {
        let mut registration = registration(username);
        registration["organizationName"] = json!("");
        registration
    };
    let address = "203.0.113.30";
    for username in ["dora", "erin"] {
        post(&th, REGISTER, &alone(username), of(address))
            .await
            .check_created();
    }
    post(&th, REGISTER, &alone("fay"), of(address))
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);

    th.teardown().await;
    smtp.stop().await;
}
