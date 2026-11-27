//! Who an identity tied by a provider's move is, its address or its subject, is told
//! in the organization's own records, which its administrators alone read: not to the
//! member who carried it over, not to a guest, not in a workspace's records, and not
//! to the administrator of another organization.

use fvoci_model::{OrgRole, WorkspaceRole};
use fvoci_store::DbType;
use reqwest::StatusCode;
use serde_json::Value;

use crate::common::{ApiResponse, Client, ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID, TestServer};
use crate::fake_oidc::{FakeProvider, Person};
use crate::sso_carry_over::{Moved, Visitor, held_page, moved};
use crate::workspaces::another_organization;

/// Who the new provider says ada is.
const ADA_NEW: &str = "ada-subject-at-the-new-provider";
const ADDRESS: &str = "ada.carried@new.example.com";

/// Whether an answer tells the address, anywhere in it.
fn tells(response: &ApiResponse) -> bool {
    response.value().to_string().contains(ADDRESS)
}

/// Takes `user_id` out of the organization of the test server: no member of it, and
/// in none of its workspaces.
async fn out_of_the_organization(ts: &TestServer, user_id: &str) {
    let store = ts.app.store().system();
    for table in ["organization_members", "workspace_members"] {
        store
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
            .await
            .expect("out of the organization");
    }
}

/// A new account that is a guest of the test workspace alone, signed in.
async fn guest_of_the_test_workspace(ts: &TestServer) -> Client {
    let guest = ts.add_user("guest").await;
    let guest_id = guest.me().await.id;
    out_of_the_organization(ts, &guest_id).await;
    ts.app
        .store()
        .add_workspace_member(TEST_WORKSPACE_ID, &guest_id, WorkspaceRole::Guest)
        .await
        .expect("a guest of the test workspace");
    guest
}

/// What `client` is answered by the organization's records and by those of each
/// workspace: the status of each, and whether any tells the address.
async fn what_is_told(client: &Client, org_id: &str) -> Vec<(String, StatusCode, bool)> {
    let mut told = Vec::new();
    for path in [
        format!("/org/{org_id}/audit"),
        format!("/w/{ROOT_WORKSPACE_ID}/audit"),
        format!("/w/{TEST_WORKSPACE_ID}/audit"),
    ] {
        let response = client.get(&path).await;
        told.push((path, response.status, tells(&response)));
    }
    told
}

/// Ada carries her account over to a new identity the new provider gave the address
/// [`ADDRESS`]: the organization's administrator reads it under the record; ada
/// herself, a guest and the workspaces' records do not tell it, and on PostgreSQL,
/// where organizations share the server, neither does the administrator of another.
#[tokio::test]
async fn the_identity_carried_over_is_told_to_the_organizations_administrators_alone() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, .. } = moved(&old, &new).await;
    new.sign_in_as(Person::new(ADA_NEW, ADDRESS));
    let browser = Visitor::new(&ts, &[&old, &new]);
    assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
    assert_eq!(browser.carry_over(&org_id).await, Ok("/".to_owned()));
    let ada = browser.session(&ts).expect("a session is set");
    let guest = guest_of_the_test_workspace(&ts).await;

    let records: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    let told: Vec<(&str, &str)> = records
        .iter()
        .filter(|record| !record["identity"].is_null())
        .map(|record| {
            (
                record["action"].as_str().unwrap_or_default(),
                record["identity"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        told,
        [("oidc_identity_carried_over", ADDRESS)],
        "the administrator reads who the identity is, under its record alone"
    );
    let audit = format!("/org/{org_id}/audit");
    for (who, client) in [("ada", &ada), ("a guest", &guest)] {
        for (path, status, told) in what_is_told(client, &org_id).await {
            assert!(!told, "{who} is told the address by {path} ({status})");
            if path == audit {
                assert_eq!(status, StatusCode::NOT_FOUND, "{who}: {path}");
            }
        }
    }
    for (path, status, told) in what_is_told(&ts.client, &org_id).await {
        assert!(
            path == audit || !told,
            "a workspace's records tell the address: {path} ({status})"
        );
    }
    if fvoci_store::testing::db_type_of_the_tests() == DbType::Postgres {
        let (theirs, _, _) = another_organization(&ts).await;
        let their_admin = ts.add_user("their.admin").await;
        let their_admin_id = their_admin.me().await.id;
        out_of_the_organization(&ts, &their_admin_id).await;
        theirs
            .add_organization_member(&their_admin_id, OrgRole::Admin)
            .await
            .expect("the administrator of their organization");
        let their_org = theirs.tenant().org_id().expect("their organization");
        for (path, status, told) in what_is_told(&their_admin, &org_id).await {
            assert!(!told, "their administrator is told by {path} ({status})");
        }
        let their_records = their_admin.get(&format!("/org/{their_org}/audit")).await;
        assert_eq!(their_records.status, StatusCode::OK);
        assert!(
            !tells(&their_records),
            "their own records tell nothing of ours"
        );
        assert_eq!(
            their_admin.get(&audit).await.status,
            StatusCode::NOT_FOUND,
            "ours are not theirs to read"
        );
    }
    ts.teardown().await;
    old.stop();
    new.stop();
}
