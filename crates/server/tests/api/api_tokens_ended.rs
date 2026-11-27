//! What ends an account's sessions ends its personal API tokens, in the same change:
//! a password reset by link or by the operator, asking to delete the account, and a
//! provider removed, changed or unlinked for those made through it. A password
//! changed, the address given back and the account erased are with their own tests
//! (`api_tokens`, `email_revert`, `account_erasure`); a first password, in the store's.

use reqwest::StatusCode;
use serde_json::json;

use crate::api_tokens::{agent_reading, check_token_ended};
use crate::common::{PASSWORD, ROOT_WORKSPACE_ID, Setup, USER2_USERNAME};
use crate::fake_oidc::{CLIENT_SECRET, FakeProvider};
use crate::oidc::{org_of, server_for, set_provider};
use crate::sso::{ADMIN_SUBJECT, admin_through_provider, through_provider};

#[tokio::test]
async fn a_reset_password_ends_the_accounts_tokens_and_no_one_elses() {
    let ts = Setup::native().start().await.init_basic().await;
    let first = agent_reading(&ts, &ts.client).await;
    let second = agent_reading(&ts, &ts.client2).await;

    let url = ts
        .app
        .password_reset_link("user1")
        .await
        .expect("link issued");
    let token = url.split_once('#').expect("a fragment").1;
    ts.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_no_content();
    check_token_ended(&first).await;
    second
        .get("/w/00000000-0000-7000-8000-000000000000/boards")
        .await
        .check_ok();

    ts.app
        .reset_user_password(USER2_USERNAME, "Operator-password-1")
        .await
        .expect("the operator resets the password");
    check_token_ended(&second).await;

    ts.teardown().await;
}

#[tokio::test]
async fn asking_to_delete_the_account_ends_its_tokens() {
    let ts = Setup::native().start().await.init_basic().await;
    let agent = agent_reading(&ts, &ts.client2).await;
    ts.client2
        .post(
            "/users/me/deletion",
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();
    check_token_ended(&agent).await;

    ts.teardown().await;
}

#[tokio::test]
async fn a_provider_changed_or_removed_ends_the_tokens_made_through_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let provider = format!("/org/{org_id}/oidc");
    let with_password = agent_reading(&ts, &ts.client).await;

    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let changed = agent_reading(&ts, &through).await;
    ts.client
        .put(
            &provider,
            &json!({"issuer": fake.issuer, "clientId": "another-client", "clientSecret": CLIENT_SECRET}),
        )
        .await
        .check_ok();
    check_token_ended(&changed).await;
    // The session made through it stays signed in, and makes no token: a refusal that
    // is no 401, which the app would take for being signed out.
    let refused = through
        .post(
            "/users/me/api-tokens",
            &json!({"name": "agent", "scope": "read", "workspaceIds": [ROOT_WORKSPACE_ID]}),
        )
        .await;
    refused.check_status(StatusCode::CONFLICT);
    assert_eq!(
        refused.value()["reason"],
        "provider_changed",
        "{}",
        refused.text()
    );
    through.get("/users/me").await.check_ok();

    set_provider(&ts, &org_id, &fake).await;
    let through = through_provider(&ts, &fake, &org_id, ADMIN_SUBJECT).await;
    let removed = agent_reading(&ts, &through).await;
    ts.client.delete(&provider).await.check_no_content();
    check_token_ended(&removed).await;

    with_password
        .get("/w/00000000-0000-7000-8000-000000000000/boards")
        .await
        .check_ok();
    ts.teardown().await;
    fake.stop();
}

#[tokio::test]
async fn an_identity_unlinked_ends_the_tokens_made_through_its_provider() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let with_password = agent_reading(&ts, &ts.client).await;
    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let agent = agent_reading(&ts, &through).await;

    ts.client
        .delete_json(
            "/users/me/identities",
            &json!({"issuer": fake.issuer, "subject": ADMIN_SUBJECT, "password": PASSWORD}),
        )
        .await
        .check_no_content();
    check_token_ended(&agent).await;
    with_password
        .get("/w/00000000-0000-7000-8000-000000000000/boards")
        .await
        .check_ok();

    ts.teardown().await;
    fake.stop();
}
