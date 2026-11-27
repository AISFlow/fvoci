//! Moving an organization to another provider: the provider before is kept as the
//! former one for the configured time, shown to the administrators, ended by one of
//! them before its time or by the housekeeping when it ran out, and all of it is in
//! the organization's records.

use fvoci_model::OidcProvider;
use fvoci_server::config::Configuration;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{Client, Setup, TestServer};
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider};
use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};

/// Thirty days, in milliseconds: the default of `oidc_former_provider_days`.
const THIRTY_DAYS: i64 = 30 * 24 * 60 * 60 * 1000;

/// A server that reaches both providers, configured as `configure` says beside.
pub(crate) async fn server_for_two(
    old: &FakeProvider,
    new: &FakeProvider,
    configure: fn(&mut Configuration),
) -> TestServer {
    Setup::native()
        .trusting_oidc_provider(&old.certificate_der)
        .trusting_oidc_provider(&new.certificate_der)
        .configure(with_key_and_private_addresses)
        .configure(configure)
        .start()
        .await
        .init_basic()
        .await
}

/// The settings of the fake provider `fake`, for a save.
pub(crate) fn settings_of(fake: &FakeProvider) -> Value {
    json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET })
}

/// The organization's own records, newest first: the action, the kind of the actor,
/// and the issuer named.
pub(crate) async fn records(client: &Client, org_id: &str) -> Vec<(String, String, String)> {
    let records: Vec<Value> = client.get(&format!("/org/{org_id}/audit")).await.ok_json();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    records
        .iter()
        .map(|record| {
            (
                text(&record["action"]),
                text(&record["actor"]["kind"]),
                text(&record["detail"]),
            )
        })
        .collect()
}

/// A move to another issuer keeps the provider before for thirty days, without its
/// secret in any answer; an administrator ends that before its time, once, naming the
/// one they read, and nobody else does.
#[tokio::test]
async fn the_provider_before_is_kept_and_ended_by_an_administrator() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let ts = server_for_two(&old, &new, |_| {}).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let first = set_provider(&ts, &org_id, &old).await;

    let moved = ts.client.put(&path, &settings_of(&new)).await;
    let read: OidcProvider = ts.client.get(&path).await.ok_json();
    let kept_since = read.former.as_ref().map_or(0, |former| former.created_at);
    let end = async |client: &Client, created_at: i64| {
        Client::send(
            client
                .request(reqwest::Method::DELETE, &format!("{path}/former"))
                .json(&json!({ "createdAt": created_at })),
        )
        .await
    };
    let by_a_member = end(&ts.client2, kept_since).await;
    let of_another = end(&ts.client, kept_since - 1).await;
    let unnamed = ts.client.delete(&format!("{path}/former")).await;
    let ended = end(&ts.client, kept_since).await;
    let again = end(&ts.client, kept_since).await;
    let after: OidcProvider = ts.client.get(&path).await.ok_json();

    assert_eq!(first.former, None, "the first provider took no one's place");
    assert!(!moved.text().contains(CLIENT_SECRET), "{}", moved.text());
    let moved: OidcProvider = moved.ok_json();
    let former = moved.former.as_ref().expect("the provider before is kept");
    assert_eq!(former.issuer, old.issuer);
    assert_eq!(former.expires_at - former.created_at, THIRTY_DAYS);
    assert_eq!(moved.issuer, new.issuer);
    assert_eq!(read.former, moved.former);
    by_a_member.check_status(StatusCode::NOT_FOUND);
    of_another.check_status(StatusCode::CONFLICT);
    unnamed.check_status(StatusCode::BAD_REQUEST);
    ended.check_no_content();
    again.check_no_content();
    assert_eq!(after.former, None);
    let person = |action: &str| (action.to_owned(), "person".to_owned(), old.issuer.clone());
    let recorded = records(&ts.client, &org_id).await;
    let of_the_former: Vec<_> = recorded
        .iter()
        .filter(|(action, ..)| action.starts_with("oidc_former_provider_"))
        .cloned()
        .collect();
    assert_eq!(
        of_the_former,
        [
            person("oidc_former_provider_ended"),
            person("oidc_former_provider_kept")
        ],
        "ended once"
    );
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// A former provider whose time ran out is no more shown, and the housekeeping
/// removes it and records that as the server's own doing.
#[tokio::test]
async fn one_whose_time_ran_out_is_gone_and_recorded_as_the_servers() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let ts = server_for_two(&old, &new, |_| {}).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    set_provider(&ts, &org_id, &old).await;
    ts.client.put(&path, &settings_of(&new)).await.check_ok();
    ts.app
        .store()
        .system()
        .execute_unchecked("UPDATE oidc_former_providers SET expires_at = 1")
        .await
        .expect("its time ran out");

    let read: OidcProvider = ts.client.get(&path).await.ok_json();
    let before_the_housekeeping = records(&ts.client, &org_id).await;
    fvoci_server::keep_house(&ts.app).await;
    let recorded = records(&ts.client, &org_id).await;

    assert_eq!(read.former, None, "read as none before it is removed");
    assert!(
        !before_the_housekeeping
            .iter()
            .any(|(action, ..)| action == "oidc_former_provider_lapsed")
    );
    assert_eq!(
        recorded[0],
        (
            "oidc_former_provider_lapsed".to_owned(),
            "server".to_owned(),
            old.issuer.clone()
        )
    );
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// With `oidc_former_provider_days` zero a move keeps nothing.
#[tokio::test]
async fn no_days_keep_no_former_provider() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let ts = server_for_two(&old, &new, |config| config.oidc_former_provider_days = 0).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &old).await;

    let moved: OidcProvider = ts
        .client
        .put(&format!("/org/{org_id}/oidc"), &settings_of(&new))
        .await
        .ok_json();

    assert_eq!(moved.former, None);
    ts.teardown().await;
    old.stop();
    new.stop();
}
