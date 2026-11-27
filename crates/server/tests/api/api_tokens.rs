//! Personal API tokens over HTTP: made by a session and told once, opening what they
//! were given on the routes that take them, refused everywhere else, and ended with
//! their owner's sessions.

use fvoci_model::{ContainerVisibility, IssuedApiToken, UserWorkspace};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, PASSWORD, ROOT_WORKSPACE_ID, Setup,
    TEST_WORKSPACE_ID, TestServer,
};
use crate::mcp::{called, listed, refused};

/// Makes a token as `client` that reaches `workspaces`, with `scope`.
async fn make_token(client: &Client, scope: &str, workspaces: &[&str]) -> IssuedApiToken {
    client
        .post(
            "/users/me/api-tokens",
            &json!({"name": "agent", "scope": scope, "workspaceIds": workspaces}),
        )
        .await
        .created_json()
}

/// A client that sends `token` as its bearer token.
fn agent(ts: &TestServer, issued: &IssuedApiToken) -> Client {
    ts.client_with_token(&issued.token)
}

/// Checks a refusal: the status and the reason.
#[track_caller]
fn check_refused(response: &ApiResponse, status: StatusCode, reason: &str) {
    assert_eq!(
        response.status,
        status,
        "{} {}",
        response.path,
        response.text()
    );
    assert_eq!(
        response.error_reason(),
        reason,
        "{} {}",
        response.path,
        response.text()
    );
}

/// Makes a card on the board as `client`.
async fn create_card(client: &Client, workspace_id: &str, board_id: &str) -> ApiResponse {
    client
        .post(
            &format!("/w/{workspace_id}/boards/{board_id}/cards"),
            &json!({"title": "by the agent"}),
        )
        .await
}

#[tokio::test]
async fn a_token_is_told_once_and_listed_by_its_prefix_alone() {
    let ts = Setup::native().start().await.init_basic().await;
    let issued = make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    assert!(issued.token.starts_with("fvoci_pat_"), "{}", issued.token);
    assert!(issued.token.starts_with(&issued.api_token.token_prefix));
    assert_eq!(
        issued.api_token.expires_at - issued.api_token.created_at,
        90 * 86_400_000
    );

    let listed: Value = ts.client.get("/users/me/api-tokens").await.ok_json();
    let text = listed.to_string();
    assert!(
        !text.contains(&issued.token),
        "the token is never listed: {text}"
    );
    assert_eq!(listed[0]["id"], issued.api_token.id.as_str());
    assert_eq!(listed[0]["workspaces"][0]["workspaceId"], TEST_WORKSPACE_ID);
    let stored = ts
        .app
        .store()
        .texts_unchecked("SELECT token_hash FROM api_tokens")
        .await
        .expect("read");
    assert!(stored.iter().all(|hash| !hash.contains(&issued.token)));

    let others: Value = ts.client2.get("/users/me/api-tokens").await.ok_json();
    assert_eq!(others, json!([]), "another's are not listed");
    ts.client2
        .delete(&format!("/users/me/api-tokens/{}", issued.api_token.id))
        .await
        .check_status(StatusCode::NOT_FOUND);

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_is_given_only_what_its_maker_works_in() {
    let ts = Setup::native().start().await.init_basic().await;
    for (body, field) in [
        (json!({"name": "agent", "scope": "read"}), "/workspaceIds"),
        (
            json!({"name": " ", "scope": "read", "workspaceIds": [TEST_WORKSPACE_ID]}),
            "/name",
        ),
        (
            json!({"name": "a", "scope": "read", "workspaceIds": [TEST_WORKSPACE_ID], "expiresInDays": 366}),
            "/expiresInDays",
        ),
    ] {
        let refused = ts.client.post("/users/me/api-tokens", &body).await;
        refused.check_status(StatusCode::BAD_REQUEST);
        assert_eq!(refused.refused_at(), field, "{}", refused.text());
    }
    ts.client
        .post(
            "/users/me/api-tokens",
            &json!({"name": "a", "scope": "read", "workspaceIds": [HIDDEN_WORKSPACE_ID]}),
        )
        .await
        .check_status(StatusCode::NOT_FOUND);

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_reads_and_writes_the_content_it_was_given_and_nothing_else() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let elsewhere = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let writer = agent(
        &ts,
        &make_token(&ts.client, "write", &[TEST_WORKSPACE_ID]).await,
    );
    let reader = agent(
        &ts,
        &make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await,
    );

    let boards = format!("/w/{TEST_WORKSPACE_ID}/boards");
    writer.get(&boards).await.check_ok();
    reader.get(&boards).await.check_ok();
    let card: Value = create_card(&writer, TEST_WORKSPACE_ID, &board.id)
        .await
        .created_json();
    let card_path = format!(
        "/w/{TEST_WORKSPACE_ID}/cards/{}",
        card["id"].as_str().expect("id")
    );
    reader.get(&card_path).await.check_ok();
    check_refused(
        &create_card(&reader, TEST_WORKSPACE_ID, &board.id).await,
        StatusCode::FORBIDDEN,
        "token_scope_insufficient",
    );

    // A workspace the token was not given is not there for it, though its owner works
    // in it.
    ts.client
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    writer
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_status(StatusCode::NOT_FOUND);
    create_card(&writer, ROOT_WORKSPACE_ID, &elsewhere.id)
        .await
        .check_status(StatusCode::NOT_FOUND);

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_is_refused_by_what_is_the_accounts_the_peoples_and_the_administrators() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let writer = agent(
        &ts,
        &make_token(&ts.client, "write", &[TEST_WORKSPACE_ID]).await,
    );
    let workspaces: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    let org_id = &workspaces[0].org_id;
    let ws = format!("/w/{TEST_WORKSPACE_ID}");
    let container = format!("{ws}/containers/{}", board.id);
    let forbidden = [
        writer.get("/users/me").await,
        writer.get("/users/me/workspaces").await,
        writer.get("/users/me/api-tokens").await,
        writer
            .post(
                "/users/me/api-tokens",
                &json!({"name": "more", "scope": "write"}),
            )
            .await,
        writer.get(&format!("/org/{org_id}")).await,
        writer.get(&format!("/org/{org_id}/api-tokens")).await,
        writer.get(&format!("{ws}/invitations")).await,
        writer
            .post(&format!("{ws}/invitations"), &json!({"role": "member"}))
            .await,
        writer.get(&format!("{container}/public-link")).await,
        writer.get(&format!("{container}/members")).await,
        writer.get("/notifications").await,
        writer.get("/notifications/unread").await,
        writer.get("/users/me/time/active").await,
        writer.get("/users/me/time/entries").await,
        writer.post("/users/me/time/start", &json!({})).await,
        writer.get(&format!("{ws}/categories")).await,
        writer
            .post(&format!("{ws}/boards"), &json!({"title": "made"}))
            .await,
        writer.delete(&container).await,
        writer.post_empty("/logout").await,
    ];
    for refused in &forbidden {
        check_refused(refused, StatusCode::FORBIDDEN, "token_forbidden_endpoint");
    }
    // Nor does it follow anything: the websocket is a person's, refused as any route
    // of a person is.
    assert_eq!(
        writer.try_connect_websocket(&[]).await.err(),
        Some(StatusCode::FORBIDDEN)
    );
    // In the cookie it is no token: a page cannot send one by itself.
    let in_a_cookie = writer.as_browser();
    check_refused(
        &in_a_cookie.get(&format!("{ws}/boards")).await,
        StatusCode::FORBIDDEN,
        "token_forbidden_endpoint",
    );

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_ends_when_taken_back_or_with_its_owners_password() {
    let ts = Setup::native().start().await.init_basic().await;
    let boards = format!("/w/{TEST_WORKSPACE_ID}/boards");
    let taken_back = make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    let kept = make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await;
    ts.client
        .delete(&format!("/users/me/api-tokens/{}", taken_back.api_token.id))
        .await
        .check_no_content();
    agent(&ts, &taken_back)
        .get(&boards)
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    agent(&ts, &kept).get(&boards).await.check_ok();

    ts.client
        .post(
            "/users/me/password",
            &json!({"oldPassword": PASSWORD, "newPassword": "another password"}),
        )
        .await
        .check_no_content();
    agent(&ts, &kept)
        .get(&boards)
        .await
        .check_status(StatusCode::UNAUTHORIZED);

    ts.teardown().await;
}

#[tokio::test]
async fn a_token_stops_reaching_a_workspace_its_owner_leaves() {
    let ts = Setup::native().start().await.init_basic().await;
    let user2 = ts.client2.me().await;
    let reader = agent(
        &ts,
        &make_token(&ts.client2, "read", &[TEST_WORKSPACE_ID]).await,
    );
    let boards = format!("/w/{TEST_WORKSPACE_ID}/boards");
    reader.get(&boards).await.check_ok();
    let user1 = ts.client.me().await;
    ts.app
        .store()
        .remove_workspace_member(TEST_WORKSPACE_ID, &user2.id, &user1.id)
        .await
        .expect("user2 leaves the workspace");
    reader
        .get(&boards)
        .await
        .check_status(StatusCode::NOT_FOUND);

    ts.teardown().await;
}

#[tokio::test]
async fn an_organization_that_takes_no_password_gives_its_workspaces_to_tokens_while_it_lets() {
    use crate::fake_oidc::FakeProvider;
    use crate::oidc::{org_of, server_for, set_provider};
    use crate::sso::{Passwords, admin_through_provider, require_sso};

    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    let asked = json!({
        "name": "agent", "scope": "read", "workspaceIds": [ROOT_WORKSPACE_ID],
        "expiresInDays": 365,
    });
    let org = format!("/org/{org_id}");
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");

    check_refused(
        &admin.post("/users/me/api-tokens", &asked).await,
        StatusCode::FORBIDDEN,
        "api_tokens_disallowed",
    );
    admin
        .patch(&org, &json!({"allowApiTokens": true}))
        .await
        .check_ok();
    // A session made with a password does not open the organization, nor gives it.
    check_refused(
        &ts.client.post("/users/me/api-tokens", &asked).await,
        StatusCode::UNAUTHORIZED,
        "sso_required",
    );
    let issued: IssuedApiToken = admin
        .post("/users/me/api-tokens", &asked)
        .await
        .created_json();
    assert_eq!(
        issued.api_token.expires_at - issued.api_token.created_at,
        90 * 86_400_000,
        "cut to the 90 days of such an organization"
    );
    let reader = agent(&ts, &issued);
    reader.get(&boards).await.check_ok();
    check_given_to_the_agent(&reader).await;

    // Switched off, the token opens nothing of the organization until it is on again.
    admin
        .patch(&org, &json!({"allowApiTokens": false}))
        .await
        .check_ok();
    check_refused(
        &reader.get(&boards).await,
        StatusCode::FORBIDDEN,
        "api_tokens_disallowed",
    );
    check_kept_from_the_agent(&reader).await;
    admin
        .patch(&org, &json!({"allowApiTokens": true}))
        .await
        .check_ok();
    reader.get(&boards).await.check_ok();

    // Its administrators see it, and take it back.
    let listed: Value = admin.get(&format!("{org}/api-tokens")).await.ok_json();
    assert_eq!(listed[0]["id"], issued.api_token.id.as_str(), "{listed}");
    ts.client2
        .get(&format!("{org}/api-tokens"))
        .await
        .check_status(StatusCode::NOT_FOUND);
    admin
        .delete(&format!("{org}/api-tokens/{}", issued.api_token.id))
        .await
        .check_no_content();
    reader
        .get(&boards)
        .await
        .check_status(StatusCode::UNAUTHORIZED);

    // The organization's records say who let tokens in, stopped them and took one out.
    let records: Vec<fvoci_model::AuditRecord> = admin.get(&format!("{org}/audit")).await.ok_json();
    let tokens: Vec<_> = records
        .iter()
        .filter_map(|record| {
            let action = record.action.as_str();
            action.contains("api_token").then(|| {
                let subject = record.subject.as_ref().map(|subject| subject.id.as_str());
                (action, subject, record.detail.as_deref())
            })
        })
        .collect();
    let owner = issued.api_token.user_id.as_str();
    let prefix = issued.api_token.token_prefix.as_str();
    assert_eq!(
        tokens,
        [
            ("api_token_revoked", Some(owner), Some(prefix)),
            ("api_tokens_enabled", None, None),
            ("api_tokens_disabled", None, None),
            ("api_tokens_enabled", None, None),
        ],
        "{records:?}"
    );

    ts.teardown().await;
}

/// A token made to last a year before its organization took only sessions made through
/// its provider still opens it while it is younger than the 90 days such an
/// organization gives a token, and not after.
#[tokio::test]
async fn a_token_made_before_an_organization_took_only_its_provider_lasts_its_90_days_there() {
    use crate::fake_oidc::FakeProvider;
    use crate::oidc::{org_of, server_for, set_provider};
    use crate::sso::{Passwords, admin_through_provider, require_sso};

    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    let issued: IssuedApiToken = admin
        .post(
            "/users/me/api-tokens",
            &json!({
                "name": "agent", "scope": "read", "workspaceIds": [ROOT_WORKSPACE_ID],
                "expiresInDays": 365,
            }),
        )
        .await
        .created_json();
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    admin
        .patch(&format!("/org/{org_id}"), &json!({"allowApiTokens": true}))
        .await
        .check_ok();
    let reader = agent(&ts, &issued);
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    reader.get(&boards).await.check_ok();

    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE api_tokens SET created_at = created_at - {} WHERE id = '{}'",
            91 * 86_400_000_i64,
            issued.api_token.id
        ))
        .await
        .expect("the token is older");
    check_refused(
        &reader.get(&boards).await,
        StatusCode::UNAUTHORIZED,
        "token_expired_for_organization",
    );

    ts.teardown().await;
}

/// A token of `client`'s user that reads the root workspace, as an agent holds it.
pub(crate) async fn agent_reading(ts: &TestServer, client: &Client) -> Client {
    let issued = make_token(client, "read", &[ROOT_WORKSPACE_ID]).await;
    let agent = agent(ts, &issued);
    agent
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    agent
}

/// Checks that the agent's token opens nothing any more.
pub(crate) async fn check_token_ended(agent: &Client) {
    agent
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_token_is_held_to_its_requests_a_minute_and_its_owner_is_not() {
    let ts = Setup::native().start().await.init_basic().await;
    let boards = format!("/w/{TEST_WORKSPACE_ID}/boards");
    let held = agent(
        &ts,
        &make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await,
    );
    let other = agent(
        &ts,
        &make_token(&ts.client, "read", &[TEST_WORKSPACE_ID]).await,
    );
    for _ in 0..120 {
        held.get(&boards).await.check_ok();
    }
    let refused = held.get(&boards).await;
    check_refused(
        &refused,
        StatusCode::TOO_MANY_REQUESTS,
        "token_rate_limited",
    );
    let wait: u64 = refused
        .headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("a wait in seconds");
    assert!((1..=60).contains(&wait), "{wait}");
    other.get(&boards).await.check_ok();
    ts.client.get(&boards).await.check_ok();

    ts.teardown().await;
}

/// An upload of an archive is its opener's, as a person: their token, which opens
/// their uploads of files, is refused on it, and the upload is left as it was.
#[tokio::test]
async fn a_token_is_refused_an_upload_of_an_archive() {
    let ts = Setup::native().start().await.init_basic().await;
    let opened = ts
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/archive/uploads"),
            &json!({"name": "archive.fvoci", "size": 10}),
        )
        .await;
    opened.check_created();
    let upload = format!(
        "/w/{TEST_WORKSPACE_ID}/uploads/{}",
        opened.value()["uploadId"].as_str().expect("an upload")
    );
    let agent = agent_writing(&ts, &ts.client).await;
    for refused in [
        agent.get(&upload).await,
        agent.post(&format!("{upload}/complete"), &json!({})).await,
        agent.delete(&upload).await,
    ] {
        check_refused(&refused, StatusCode::FORBIDDEN, "token_forbidden_endpoint");
    }
    ts.client.get(&upload).await.check_ok();

    ts.teardown().await;
}

/// A token of `client`'s user that writes in the test workspace, as an agent holds it.
pub(crate) async fn agent_writing(ts: &TestServer, client: &Client) -> Client {
    agent(ts, &make_token(client, "write", &[TEST_WORKSPACE_ID]).await)
}

/// The root workspace is listed to the agent of `reader`, and its overview given.
async fn check_given_to_the_agent(reader: &Client) {
    assert_eq!(
        listed(reader).await,
        [ROOT_WORKSPACE_ID],
        "and to its agent"
    );
    let overview = json!({"workspaceId": ROOT_WORKSPACE_ID});
    called(reader, "get_workspace_overview", &overview).await;
}

/// The root workspace is not listed to the agent of `reader`, and its overview is
/// refused for the organization that lets no token in.
async fn check_kept_from_the_agent(reader: &Client) {
    assert_eq!(
        listed(reader).await,
        Vec::<String>::new(),
        "nor to its agent"
    );
    let overview = json!({"workspaceId": ROOT_WORKSPACE_ID});
    let problem = refused(reader, "get_workspace_overview", &overview).await;
    assert_eq!(problem["reason"], "api_tokens_disallowed", "{problem}");
}
