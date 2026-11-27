//! Another secret, client or issuer saved while the organization takes no password: kept
//! aside as the provider's pending settings, nobody signs in through them, and people
//! go on signing in through the provider as it is; a trial through them that ends well
//! makes them the provider's, one that fails changes nothing, and they can be thrown
//! away.

use fvoci_model::{OidcProvider, OidcStart};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, NEXT_CLIENT_SECRET, Person};
use crate::oidc::{Browser, org_of, server_for, set_provider, with_key_and_private_addresses};
use crate::sso::{
    ADMIN_SUBJECT, Passwords, admin_through_provider, require_sso, through_provider, tie,
};

/// A secret the fake provider does not take at its token endpoint.
const WRONG_SECRET: &str = "a secret the provider never issued";

/// Where a trial came back to, and the session it set if it set one.
#[derive(Debug)]
struct Tried {
    location: String,
    token: Option<String>,
}

/// Tries the pending settings from `admin`'s browser, as the organization's page does,
/// with the fake provider signing `user1` in.
async fn try_pending(ts: &TestServer, fake: &FakeProvider, admin: &Client, org_id: &str) -> Tried {
    try_as(ts, fake, admin, org_id, ADMIN_SUBJECT).await
}

/// Tries the pending settings as [`try_pending`] does, with the fake provider signing
/// in the person `subject`.
async fn try_as(
    ts: &TestServer,
    fake: &FakeProvider,
    admin: &Client,
    org_id: &str,
    subject: &str,
) -> Tried {
    try_and_come_back_with(ts, fake, admin, org_id, (subject, &admin.token)).await
}

/// Tries the pending settings from `admin`'s browser as [`try_as`] does, signing in the
/// person `subject`, and comes back from the provider with the session cookie
/// `back_with`: the browser's own, or another.
async fn try_and_come_back_with(
    ts: &TestServer,
    fake: &FakeProvider,
    admin: &Client,
    org_id: &str,
    (subject, back_with): (&str, &str),
) -> Tried {
    fake.sign_in_as(Person::new(subject, &format!("{subject}@example.com")));
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tls_certs_merge([
            reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
        ])
        .build()
        .expect("a client");
    let started = http
        .post(format!(
            "{}/api/v1/org/{org_id}/oidc/pending/sign-in",
            ts.base_url
        ))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Cookie", format!("fvoci_session={}", admin.token))
        .send()
        .await
        .expect("the server answers");
    assert_eq!(started.status(), StatusCode::OK, "the trial starts");
    let flow = cookie_of(&started, "fvoci_oidc").expect("the cookie of the sign-in");
    let started: OidcStart = started.json().await.expect("where the browser goes");
    let authorized = http
        .get(&started.authorization_url)
        .send()
        .await
        .expect("the provider answers");
    let back = location_of(&authorized);
    let root = ts.app.config().server_url.trim_end_matches('/').to_owned();
    let path = back
        .strip_prefix(&root)
        .expect("under the server's address");
    let answer = http
        .get(format!("{}{path}", ts.base_url))
        .header(
            "Cookie",
            format!("fvoci_oidc={flow}; fvoci_session={back_with}"),
        )
        .send()
        .await
        .expect("the server answers");
    assert_eq!(answer.status(), StatusCode::SEE_OTHER, "a return sends on");
    Tried {
        location: location_of(&answer)
            .strip_prefix(&root)
            .expect("under the server's address")
            .to_owned(),
        token: cookie_of(&answer, "fvoci_session"),
    }
}

fn cookie_of(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| {
            cookie
                .split(';')
                .next()?
                .strip_prefix(&format!("{name}="))
                .map(str::to_owned)
        })
        .filter(|value| !value.is_empty())
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location header")
        .to_owned()
}

/// The actions of the organization's own records, newest first.
async fn recorded(client: &Client, org_id: &str) -> Vec<String> {
    let records: Vec<Value> = client.get(&format!("/org/{org_id}/audit")).await.ok_json();
    records
        .iter()
        .map(|record| record["action"].as_str().expect("an action").to_owned())
        .collect()
}

/// An organization that takes no password, set up by `user1` through the fake provider:
/// the server, the organization and the administrator's proving session.
async fn enforced(fake: &FakeProvider) -> (TestServer, String, Client) {
    let ts = server_for(fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, fake).await;
    let admin = admin_through_provider(&ts, fake, &org_id).await;
    require_sso(&admin, &org_id, fake, Passwords::Refused)
        .await
        .check_ok();
    (ts, org_id, admin)
}

fn with_secret(fake: &FakeProvider, secret: &str) -> Value {
    json!({
        "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": secret,
        "ssoRequired": true,
    })
}

/// A wrong secret saved while the organization takes no password locks nobody out: it
/// waits aside, the provider signs people in as before, and a trial through it fails at
/// the provider and changes nothing.
#[tokio::test]
async fn a_wrong_secret_waits_aside_and_locks_nobody_out() {
    let fake = FakeProvider::start().await;
    let (ts, org_id, admin) = enforced(&fake).await;
    let path = format!("/org/{org_id}/oidc");
    let before: OidcProvider = admin.get(&path).await.ok_json();

    let saved: OidcProvider = admin
        .put(&path, &with_secret(&fake, WRONG_SECRET))
        .await
        .ok_json();
    fake.sign_in_as(Person::new(ADMIN_SUBJECT, "user1@example.com"));
    let meanwhile = Browser::new(&ts, &fake).sign_in(&org_id).await;
    let tried = try_pending(&ts, &fake, &admin, &org_id).await;
    let after: OidcProvider = admin.get(&path).await.ok_json();

    let pending = saved.pending.as_ref().expect("saved aside");
    assert!(pending.has_client_secret && pending.client_id == CLIENT_ID);
    assert!(
        saved.can_require_sso,
        "the provider as it was is still proved"
    );
    assert_eq!(
        saved.updated_at, before.updated_at,
        "the provider itself unchanged"
    );
    let fresh = ts.client_with_token(meanwhile.token());
    fresh
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending_error=oidc-token-refused")
    );
    assert_eq!(tried.token, None, "no session");
    assert_eq!(after.pending, saved.pending, "still aside");
    assert_eq!(after.updated_at, before.updated_at);
    let answered = admin.get(&path).await;
    assert!(
        !answered.text().contains(WRONG_SECRET),
        "{}",
        answered.text()
    );
    assert_eq!(
        recorded(&admin, &org_id).await[0],
        "oidc_provider_pending_saved"
    );
    ts.teardown().await;
    fake.stop();
}

/// The next secret saved aside becomes the provider's by a trial that ends well: the
/// trial's session is made at the new revision and proves it, the session made with
/// the secret before proves it no more, and the records say who saved and who applied
/// it. The stored secret sent again is no change, and sets nothing aside.
#[tokio::test]
async fn the_next_secret_is_applied_by_a_trial() {
    let fake = FakeProvider::start().await;
    let (ts, org_id, admin) = enforced(&fake).await;
    let path = format!("/org/{org_id}/oidc");
    let same: OidcProvider = admin
        .put(&path, &with_secret(&fake, CLIENT_SECRET))
        .await
        .ok_json();
    let saved: OidcProvider = admin
        .put(&path, &with_secret(&fake, NEXT_CLIENT_SECRET))
        .await
        .ok_json();

    let tried = try_pending(&ts, &fake, &admin, &org_id).await;
    let applied = ts.client_with_token(tried.token.as_deref().unwrap_or_default());
    let read: OidcProvider = applied.get(&path).await.ok_json();
    let old_session: OidcProvider = admin.get(&path).await.ok_json();

    assert_eq!(
        same.pending, None,
        "the stored secret sent again is no change"
    );
    assert!(saved.pending.is_some());
    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending=applied")
    );
    assert!(tried.token.is_some(), "the trial's session is set");
    assert_eq!(read.pending, None, "taken");
    assert!(read.sso_required, "still taking no password");
    assert!(
        read.can_require_sso,
        "the trial's session proves the provider now"
    );
    assert!(
        !old_session.can_require_sso,
        "the session made with the secret before proves it no more"
    );
    let actions = recorded(&applied, &org_id).await;
    assert_eq!(
        actions[..2],
        [
            "oidc_provider_pending_applied",
            "oidc_provider_pending_saved"
        ]
    );
    ts.teardown().await;
    fake.stop();
}

/// Pending settings are thrown away once, and no trial starts without them.
#[tokio::test]
async fn pending_settings_are_thrown_away() {
    let fake = FakeProvider::start().await;
    let (ts, org_id, admin) = enforced(&fake).await;
    let path = format!("/org/{org_id}/oidc");
    admin
        .put(&path, &with_secret(&fake, WRONG_SECRET))
        .await
        .check_ok();

    admin
        .delete(&format!("{path}/pending"))
        .await
        .check_no_content();
    admin
        .delete(&format!("{path}/pending"))
        .await
        .check_no_content();
    let read: OidcProvider = admin.get(&path).await.ok_json();
    let started = admin.post_empty(&format!("{path}/pending/sign-in")).await;

    assert_eq!(read.pending, None);
    started.check_status(StatusCode::CONFLICT);
    assert_eq!(started.error_reason(), "oidc_unavailable");
    let actions = recorded(&admin, &org_id).await;
    assert_eq!(
        actions[..2],
        [
            "oidc_provider_pending_discarded",
            "oidc_provider_pending_saved"
        ],
        "thrown away once"
    );
    ts.teardown().await;
    fake.stop();
}

/// Another issuer or client saved while the organization takes no password is set aside
/// as well, after the new issuer's discovery is read; the provider people sign in
/// through stays as it is, and the settings saved last are the pending ones. Switching
/// the provider off with them is refused.
#[tokio::test]
async fn another_issuer_or_client_waits_aside_too() {
    let fake = FakeProvider::start().await;
    let other = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .trusting_oidc_provider(&other.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    let path = format!("/org/{org_id}/oidc");

    let mut saved = Vec::new();
    for (issuer, client_id) in [(&other.issuer, CLIENT_ID), (&fake.issuer, "another-client")] {
        let provider: OidcProvider = admin
            .put(
                &path,
                &json!({ "issuer": issuer, "clientId": client_id, "clientSecret": "s" }),
            )
            .await
            .ok_json();
        saved.push(provider);
    }
    let switched_off = admin
        .put(
            &path,
            &json!({ "issuer": other.issuer, "clientId": CLIENT_ID, "clientSecret": "s", "enabled": false }),
        )
        .await;

    for (provider, (issuer, client_id)) in saved
        .iter()
        .zip([(&other.issuer, CLIENT_ID), (&fake.issuer, "another-client")])
    {
        let pending = provider.pending.as_ref().expect("set aside");
        assert_eq!(
            (&pending.issuer, pending.client_id.as_str()),
            (issuer, client_id)
        );
        assert_eq!(
            (provider.issuer.as_str(), provider.client_id.as_str()),
            (fake.issuer.as_str(), CLIENT_ID),
            "the provider as it was"
        );
        assert!(provider.sso_required);
    }
    switched_off.check_status(StatusCode::CONFLICT);
    assert_eq!(switched_off.error_reason(), "sso_enforced");
    assert_eq!(switched_off.refused_at(), "/enabled");
    admin
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    ts.teardown().await;
    fake.stop();
    other.stop();
}

/// A trial that ends in another account than the administrator's who started it puts
/// nothing in use and makes no account: settings that let only a newcomer in would
/// leave the organization without an administrator who signs in.
#[tokio::test]
async fn a_trial_in_another_account_changes_nothing() {
    let fake = FakeProvider::start().await;
    let (ts, org_id, admin) = enforced(&fake).await;
    let path = format!("/org/{org_id}/oidc");
    admin
        .put(&path, &with_secret(&fake, NEXT_CLIENT_SECRET))
        .await
        .check_ok();
    let users_before = ts.app.store().count_rows("users").await.expect("counted");

    let tried = try_as(&ts, &fake, &admin, &org_id, "a-newcomer").await;
    let read: OidcProvider = admin.get(&path).await.ok_json();
    let users_after = ts.app.store().count_rows("users").await.expect("counted");

    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending_error=oidc-trial-account")
    );
    assert_eq!(tried.token, None);
    assert!(read.pending.is_some(), "still aside");
    assert!(read.can_require_sso, "the provider as it was");
    assert_eq!(users_after, users_before, "no account made");
    ts.teardown().await;
    fake.stop();
}

/// An organization that takes no password and moves from `fake` to `other`: the
/// server, the organization, and the administrator's session made through `fake` just
/// now, with the settings of `other` saved aside.
async fn moving(fake: &FakeProvider, other: &FakeProvider) -> (TestServer, String, Client) {
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .trusting_oidc_provider(&other.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, fake).await;
    let admin = admin_through_provider(&ts, fake, &org_id).await;
    require_sso(&admin, &org_id, fake, Passwords::Refused)
        .await
        .check_ok();
    let aside =
        json!({ "issuer": other.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET });
    admin
        .put(&format!("/org/{org_id}/oidc"), &aside)
        .await
        .check_ok();
    (ts, org_id, admin)
}

/// Makes the session of `client` one that was made `minutes` earlier than it was.
async fn age(ts: &TestServer, client: &Client, minutes: i64) {
    let aged = format!(
        "UPDATE sessions SET created_at = created_at - {} WHERE token_hash = '{}'",
        minutes * 60 * 1000,
        fvoci_server::auth::secret::token_digest(&client.token)
    );
    let changed = ts.app.store().system().execute_unchecked(&aged).await;
    assert_eq!(changed.expect("aged"), 1, "the session of the client");
}

/// Who the issuer of `fake` vouches for under the administrator's subject, if anybody.
async fn bound_to(ts: &TestServer, fake: &FakeProvider) -> Option<String> {
    let identity = fvoci_store::Identity {
        issuer: &fake.issuer,
        subject: ADMIN_SUBJECT,
    };
    let owner = ts.app.store().user_of_identity(identity).await;
    owner.expect("read").map(|user| user.id)
}

fn now_millis() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch");
    i64::try_from(since.as_millis()).expect("the time fits")
}

/// Another issuer is put in use by the trial of an administrator who signed in
/// through the provider just now: their identity at the new issuer, tied to no
/// account, is tied to theirs, the settings are applied, the provider before is kept
/// for linking, the sessions made through it end, and each is recorded.
#[tokio::test]
async fn another_issuer_is_bound_and_applied_from_a_session_made_just_now() {
    let (fake, other) = (FakeProvider::start().await, FakeProvider::start().await);
    let (ts, org_id, admin) = moving(&fake, &other).await;
    let path = format!("/org/{org_id}/oidc");
    let users_before = ts.app.store().count_rows("users").await.expect("counted");
    // Four of the five minutes are gone.
    age(&ts, &admin, 4).await;
    let waiting: OidcProvider = admin.get(&path).await.ok_json();

    let tried = try_pending(&ts, &other, &admin, &org_id).await;
    let applied = ts.client_with_token(tried.token.as_deref().unwrap_or_default());
    let read: OidcProvider = applied.get(&path).await.ok_json();

    let until = waiting
        .pending
        .and_then(|pending| pending.binds_identity_until);
    let minute = 60 * 1000;
    assert!(
        until.is_some_and(|until| until > now_millis() && until < now_millis() + 2 * minute),
        "the session was made four minutes ago: {until:?}"
    );
    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending=applied")
    );
    let me = ts.client.me().await.id;
    assert_eq!(applied.me().await.id, me, "the administrator's own account");
    assert_eq!(bound_to(&ts, &other).await, Some(me), "tied to it");
    let users_after = ts.app.store().count_rows("users").await.expect("counted");
    assert_eq!(users_after, users_before, "no account made");
    assert_eq!(read.issuer, other.issuer);
    assert!(read.sso_required && read.can_require_sso && read.pending.is_none());
    assert_eq!(
        read.former.as_ref().map(|former| former.issuer.as_str()),
        Some(fake.issuer.as_str()),
        "the provider before the trial is kept for linking"
    );
    let records: Vec<Value> = applied.get(&format!("/org/{org_id}/audit")).await.ok_json();
    let bound: Vec<&Value> = records
        .iter()
        .filter(|record| record["action"] == "oidc_identity_bound")
        .collect();
    assert_eq!(bound.len(), 1, "{records:?}");
    assert_eq!(
        bound[0]["identity"],
        format!("{ADMIN_SUBJECT}@example.com"),
        "who the identity tied is, by the address the new provider gave"
    );
    let actions = recorded(&applied, &org_id).await;
    for action in [
        "oidc_identity_bound",
        "oidc_provider_pending_applied",
        "oidc_former_provider_kept",
    ] {
        assert!(
            actions.contains(&action.to_owned()),
            "{action}: {actions:?}"
        );
    }
    admin.get("/users/me").await.check_unauthorized();
    ts.teardown().await;
    fake.stop();
    other.stop();
}

/// From a session made more than five minutes before the trial, or with another
/// session's cookie on the way back, an identity that is nobody's is tied to nobody
/// and nothing is applied; after signing in through the provider again, the trial
/// from that session binds and applies.
#[tokio::test]
async fn an_older_session_binds_nothing_until_the_administrator_signs_in_again() {
    let (fake, other) = (FakeProvider::start().await, FakeProvider::start().await);
    let (ts, org_id, admin) = moving(&fake, &other).await;
    let path = format!("/org/{org_id}/oidc");
    // Another session of the same administrator, made just now: not the one the trial
    // is started from.
    let beside = through_provider(&ts, &fake, &org_id, ADMIN_SUBJECT).await;
    let another_cookie = (ADMIN_SUBJECT, beside.token.as_str());
    let with_another = try_and_come_back_with(&ts, &other, &admin, &org_id, another_cookie).await;
    age(&ts, &admin, 6).await;
    let waiting: OidcProvider = admin.get(&path).await.ok_json();

    let stale = try_pending(&ts, &other, &admin, &org_id).await;
    let after_the_refusals = (
        bound_to(&ts, &other).await,
        admin.get(&path).await.ok_json::<OidcProvider>(),
        recorded(&admin, &org_id).await,
    );
    let again = through_provider(&ts, &fake, &org_id, ADMIN_SUBJECT).await;
    let tried = try_pending(&ts, &other, &again, &org_id).await;

    let refused = format!("/org/{org_id}?oidc_pending_error=oidc-trial-stale");
    assert_eq!(
        (with_another.location, with_another.token),
        (refused.clone(), None),
        "not the session the trial was started from"
    );
    let until = waiting
        .pending
        .and_then(|pending| pending.binds_identity_until);
    assert_eq!(until, None, "the time to bind has passed");
    assert_eq!((stale.location, stale.token), (refused, None));
    let (bound, still, actions) = after_the_refusals;
    assert_eq!(bound, None, "tied to nobody");
    assert!(
        still.pending.is_some() && still.issuer == fake.issuer && still.former.is_none(),
        "nothing applied"
    );
    assert!(
        !actions.contains(&"oidc_identity_bound".to_owned()),
        "{actions:?}"
    );
    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending=applied"),
        "from the session made by signing in again"
    );
    assert_eq!(
        bound_to(&ts, &other).await,
        Some(ts.client.me().await.id),
        "tied to the administrator"
    );
    ts.teardown().await;
    fake.stop();
    other.stop();
}

/// An identity at the new issuer that is the administrator's already applies the
/// settings from a session of any age, as before; one that is another account's is
/// refused, also from a session made just now, and stays the other's.
#[tokio::test]
async fn an_identity_tied_already_needs_no_session_made_just_now() {
    let (fake, other) = (FakeProvider::start().await, FakeProvider::start().await);
    let (ts, org_id, admin) = moving(&fake, &other).await;
    let another = ts.client2.me().await.id;
    tie(&ts, &ts.client2, &other, "somebody-else").await;

    let as_another = try_as(&ts, &other, &admin, &org_id, "somebody-else").await;
    tie(&ts, &ts.client, &other, ADMIN_SUBJECT).await;
    age(&ts, &admin, 60).await;
    let tried = try_pending(&ts, &other, &admin, &org_id).await;

    assert_eq!(
        (as_another.location, as_another.token),
        (
            format!("/org/{org_id}?oidc_pending_error=oidc-trial-account"),
            None
        ),
        "another account's identity is not the trier's"
    );
    let identity = fvoci_store::Identity {
        issuer: &other.issuer,
        subject: "somebody-else",
    };
    let owner = ts.app.store().user_of_identity(identity).await;
    assert_eq!(owner.expect("read").map(|user| user.id), Some(another));
    assert_eq!(
        tried.location,
        format!("/org/{org_id}?oidc_pending=applied")
    );
    assert!(tried.token.is_some(), "the trial's session is set");
    ts.teardown().await;
    fake.stop();
    other.stop();
}
