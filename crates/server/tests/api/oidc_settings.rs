//! What the SSO settings screen may offer for this session.
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider};
use crate::oidc::{org_of, server_for, set_provider};
use crate::sso::admin_through_provider;
use fvoci_model::OidcProvider;
use serde_json::json;

#[tokio::test]
async fn the_settings_name_whether_this_session_proved_this_provider() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let provider = set_provider(&ts, &org_id, &fake).await;
    assert!(!provider.can_require_sso, "a password proved no provider");
    ts.client2.get(&path).await.check_not_found();
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    let read: OidcProvider = admin.get(&path).await.ok_json();
    assert!(read.can_require_sso);
    let changed: OidcProvider = admin
        .put(
            &path,
            &json!({
                "issuer": fake.issuer, "clientId": "another-client", "clientSecret": CLIENT_SECRET,
            }),
        )
        .await
        .ok_json();
    assert!(!changed.can_require_sso, "another client was not proved");
    let read: OidcProvider = admin.get(&path).await.ok_json();
    assert!(!read.can_require_sso);
    let restored: OidcProvider = admin
        .put(
            &path,
            &json!({
                "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET,
            }),
        )
        .await
        .ok_json();
    assert!(
        !restored.can_require_sso,
        "restoring the client creates a new provider"
    );
    ts.stop().await.expect("the server stops");
    fake.stop();
}

#[tokio::test]
async fn http_settings_cannot_claim_an_operator_actor() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    ts.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &json!({
                "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET,
                "actorKind": "server_operator", "actor": null,
            }),
        )
        .await
        .check_ok();
    let records: Vec<fvoci_model::AuditRecord> = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.action == fvoci_model::AuditAction::OidcProviderConfigured)
            .count(),
        1,
        "one provider configuration"
    );
    let me = ts.client.me().await.id;
    for record in records {
        assert!(
            matches!(&record.actor, fvoci_model::AuditRecordActor::Person(person) if person.id == me),
            "{:?}",
            record.actor
        );
    }
    ts.stop().await.expect("the server stops");
    fake.stop();
}
