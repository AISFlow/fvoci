//! Deleting an account: asking for it, the session of an account that waits, and
//! taking the asking back.

use fvoci_model::ACCOUNT_DELETION_GRACE_DAYS;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ORG_NAME, PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer,
    USER2_USERNAME,
};
use crate::fake_oidc::{CLIENT_ID, FakeProvider, Person};
use crate::oidc::{
    Browser, Outcome, org_of, server_for, set_provider, with_key_and_private_addresses,
};
use crate::own_space::{own_workspace, workspaces_and_own};
use crate::sso::{Passwords, admin_through_provider, require_sso};
use crate::websocket::{Event, Socket, subscribe_to_workspace};

/// The account's own space as the asking names it among what goes: empty here.
fn gone_space(org_id: &str) -> Value {
    json!({"id": org_id, "kind": "personal", "name": "", "boards": 0, "wikis": 0, "diaries": 0, "courses": 0, "files": 0})
}

/// How long a test waits for what the server sends.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

const DELETION: &str = "/users/me/deletion";

/// The wait, in milliseconds.
const WAIT: i64 = ACCOUNT_DELETION_GRACE_DAYS * 24 * 60 * 60 * 1000;

async fn ask(client: &Client, username: &str, password: &str) -> ApiResponse {
    client
        .post(
            DELETION,
            &json!({"username": username, "password": password}),
        )
        .await
}

/// Checks that the answer is the 401 of a session whose account waits to be deleted.
#[track_caller]
fn check_waits(response: &ApiResponse) {
    response.check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.error_reason(),
        "account_deletion_pending",
        "{}",
        response.text()
    );
}

/// Makes `user2` an administrator of the instance past the routes: the instance then
/// keeps one whatever `user1` does, and the organization is asked for itself.
async fn make_second_instance_admin(th: &TestServer) {
    let user_id = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET is_admin = true WHERE id = '{user_id}'"
        ))
        .await
        .expect("user2 administers the instance");
}

/// A member asks, is signed out everywhere, signs in again to find that nothing opens
/// but the way back, takes it, and is where they were.
#[tokio::test]
async fn a_member_asks_to_delete_the_account_and_takes_it_back_by_signing_in() {
    let th = Setup::native().start().await.init_basic().await;
    let other_device = th.login(USER2_USERNAME, PASSWORD).await;

    let before: Value = th.client2.get(DELETION).await.ok_json();
    let own = own_workspace(&th.client2).await.org_id;
    assert_eq!(
        before,
        json!({
            "graceDays": 30,
            "proof": "password",
            "lastInstanceAdmin": false,
            "soleAdminOf": [],
            "organizations": [gone_space(&own)],
        }),
        "the account's own space goes with it, and nothing else"
    );

    let misnamed = ask(&th.client2, "user1", PASSWORD).await;
    misnamed.check_bad_request();
    assert_eq!(misnamed.refused_at(), "/username");
    let wrong = ask(&th.client2, USER2_USERNAME, "not the password").await;
    wrong.check_bad_request();
    assert_eq!(
        (wrong.error_reason().as_str(), wrong.refused_at().as_str()),
        ("invalid_credentials", "/password")
    );
    th.client2.get("/users/me").await.check_ok();

    let earliest = fvoci_server::auth::now_millis() + WAIT;
    let asked = ask(&th.client2, "USER2", PASSWORD).await;
    asked.check_ok();
    let delete_after = asked.value()["deleteAfter"]
        .as_i64()
        .expect("when the account is erased");
    assert!(
        (earliest..earliest + 60_000).contains(&delete_after),
        "thirty days from now: {delete_after}"
    );
    assert!(
        asked
            .set_cookies()
            .iter()
            .any(|cookie| cookie.contains("Max-Age=0")),
        "the session cookie is cleared: {:?}",
        asked.set_cookies()
    );
    for ended in [&th.client2, &other_device] {
        let response = ended.get("/users/me").await;
        response.check_unauthorized();
        assert_eq!(response.error_reason(), "", "the session is gone");
    }

    // Signing in still works, and opens the way back only.
    let back = th.login(USER2_USERNAME, PASSWORD).await;
    check_waits(&back.get("/users/me").await);
    check_waits(&back.get(&format!("{ROOT_WS}/boards")).await);
    check_waits(&ask(&back, USER2_USERNAME, PASSWORD).await);
    // Nor what is also open to whoever has no session: the account is let in nowhere,
    // and no other account is made from its session.
    check_waits(
        &back
            .post("/invitations/accept", &json!({"token": "no-such-link"}))
            .await,
    );
    check_waits(
        &back
            .post(
                "/invitations/accept",
                &json!({"token": "no-such-link", "account": {
                    "username": "newcomer",
                    "email": "newcomer@example.com",
                    "password": PASSWORD,
                }}),
            )
            .await,
    );
    let waiting: Value = back.get(DELETION).await.ok_json();
    assert_eq!(waiting["deleteAfter"], json!(delete_after), "{waiting}");
    assert_eq!(
        waiting["requestedAt"],
        json!(delete_after - WAIT),
        "{waiting}"
    );

    back.delete(DELETION).await.check_no_content();

    let me = back.me().await;
    assert_eq!(
        (me.username.as_str(), me.deleted_at),
        (USER2_USERNAME, None)
    );
    back.get(&format!("{ROOT_WS}/boards")).await.check_ok();
    let again = back.delete(DELETION).await;
    again.check_status(StatusCode::CONFLICT);
    let after: Value = back.get(DELETION).await.ok_json();
    assert_eq!(after, before, "nothing waits any more");
    th.teardown().await;
}

/// What the account had open closes with the asking; a session of an account that
/// waits follows nothing, and may be ended.
#[tokio::test]
async fn the_session_of_an_account_that_waits_signs_out_and_follows_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let mut open = Socket::connect_as(&th.client2).await;
    subscribe_to_workspace(&th, &mut open, ROOT_WORKSPACE_ID, &user2.id).await;

    ask(&th.client2, USER2_USERNAME, PASSWORD).await.check_ok();

    assert_eq!(
        open.next(PATIENCE).await,
        Event::Closed,
        "the connection of the account that was asked to be deleted"
    );
    let back = th.login(USER2_USERNAME, PASSWORD).await;
    // The token sent on purpose: the handshake is the session's, or refused.
    let handshake = back
        .try_connect_websocket(&[("Authorization", format!("Bearer {}", back.token))])
        .await;
    assert_eq!(
        handshake.err(),
        Some(StatusCode::UNAUTHORIZED),
        "no websocket for an account that waits"
    );

    back.post_empty("/logout").await.check_no_content();
    let response = back.get(DELETION).await;
    response.check_unauthorized();
    assert_eq!(response.error_reason(), "", "the session is over");
    th.teardown().await;
}

/// A link lets nobody in while the account of who made it waits to be deleted, and
/// does again once the asking is taken back: an organization does not fill up behind
/// an administrator who is on the way out.
#[tokio::test]
async fn the_link_of_an_account_that_waits_lets_nobody_in_meanwhile() {
    let th = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&th).await;
    // The link is `user1`'s, who may go once `user2` administers everything too.
    let token = th.member_link().await;
    make_second_instance_admin(&th).await;
    let user2 = th.client2.me().await.id;
    th.client
        .put(
            &format!("/org/{org_id}/members/{user2}/role"),
            &json!({"role": "admin"}),
        )
        .await
        .check_no_content();
    let link = json!({"token": token});
    th.anonymous()
        .post("/invitations/preview", &link)
        .await
        .check_ok();

    ask(&th.client, "user1", PASSWORD).await.check_ok();

    th.anonymous()
        .post("/invitations/preview", &link)
        .await
        .check_not_found();
    th.client2
        .post("/invitations/accept", &link)
        .await
        .check_not_found();
    let newcomer = json!({
        "token": token,
        "account": {
            "username": "newcomer",
            "email": "newcomer@sample.com",
            "password": PASSWORD,
        },
    });
    th.anonymous()
        .post("/invitations/accept", &newcomer)
        .await
        .check_not_found();
    // The member who waits is a member still, and shown as one.
    let members: Vec<Value> = th
        .client2
        .get(&format!("/org/{org_id}/members"))
        .await
        .ok_json();
    assert_eq!(members.len(), 2, "{members:?}");

    let back = th.login("user1", PASSWORD).await;
    back.delete(DELETION).await.check_no_content();
    th.anonymous()
        .post("/invitations/preview", &link)
        .await
        .check_ok();
    th.anonymous()
        .post("/invitations/accept", &newcomer)
        .await
        .check_ok();
    th.teardown().await;
}

/// The server keeps an administrator, and an organization with other people in it
/// keeps one: each refusal says which it is, and the organization.
#[tokio::test]
async fn the_last_administrator_is_refused_and_told_of_what() {
    let th = Setup::native().start().await.init_basic().await;
    let org_id = org_of(&th).await;

    let standing: Value = th.client.get(DELETION).await.ok_json();
    assert_eq!(standing["lastInstanceAdmin"], json!(true), "{standing}");
    assert_eq!(
        standing["soleAdminOf"][0]["id"],
        json!(org_id),
        "{standing}"
    );
    let of_instance = ask(&th.client, "user1", PASSWORD).await;
    of_instance.check_status(StatusCode::CONFLICT);
    assert_eq!(of_instance.error_reason(), "last_admin");

    make_second_instance_admin(&th).await;
    let of_organization = ask(&th.client, "user1", PASSWORD).await;
    of_organization.check_status(StatusCode::CONFLICT);
    assert_eq!(of_organization.error_reason(), "last_org_admin");
    assert_eq!(of_organization.value()["orgId"], json!(org_id));
    th.client.get("/users/me").await.check_ok();

    // With another administrator of the organization, the first may go.
    let user2 = th.client2.me().await.id;
    th.client
        .put(
            &format!("/org/{org_id}/members/{user2}/role"),
            &json!({"role": "admin"}),
        )
        .await
        .check_no_content();
    ask(&th.client, "user1", PASSWORD).await.check_ok();
    // And the one left is then the last, of both.
    let left = ask(&th.client2, USER2_USERNAME, PASSWORD).await;
    left.check_status(StatusCode::CONFLICT);
    assert_eq!(left.error_reason(), "last_admin");
    th.teardown().await;
}

/// An account without a password asks from a session made through its provider just
/// now, and signing in through the provider while it waits makes no second account and
/// opens the way back.
#[tokio::test]
async fn an_account_without_a_password_asks_from_a_sign_in_made_just_now() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let ada = client.me().await.id;

    let standing: Value = client.get(DELETION).await.ok_json();
    assert_eq!(standing["proof"], json!("sign_in"), "{standing}");

    // A session made a while ago proves nothing.
    ts.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE sessions SET created_at = created_at - 600000 WHERE user_id = '{ada}'"
        ))
        .await
        .expect("the session is aged");
    let stale = ask(&client, "ada", "").await;
    stale.check_forbidden();
    assert_eq!(stale.error_reason(), "sign_in_again");

    let fresh = ts.client_with_token(browser.sign_in(&org_id).await.token());
    ask(&fresh, "ada", "").await.check_ok();
    check_unauthorized_plainly(&fresh.get("/users/me").await);

    // Signing in through the provider meanwhile: the same account, waiting.
    let users = ts.app.store().count_rows("users").await.expect("counted");
    let back = ts.client_with_token(browser.sign_in(&org_id).await.token());
    check_waits(&back.get("/users/me").await);
    assert_eq!(
        ts.app.store().count_rows("users").await.expect("counted"),
        users,
        "no second account"
    );
    back.delete(DELETION).await.check_no_content();
    assert_eq!(back.me().await.id, ada);
    let (workspaces, _own) = workspaces_and_own(&back).await;
    assert_eq!(workspaces.len(), 1, "a member of what they were");
    ts.teardown().await;
    fake.stop();
}

#[track_caller]
fn check_unauthorized_plainly(response: &ApiResponse) {
    response.check_unauthorized();
    assert_eq!(response.error_reason(), "", "{}", response.text());
}

/// The address of an account that waits is taken still: who signs in through a
/// provider with it is told to sign in to the account, and gets no second one.
#[tokio::test]
async fn the_address_of_an_account_that_waits_makes_no_second_account() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    ask(&ts.client2, USER2_USERNAME, PASSWORD).await.check_ok();
    let users = ts.app.store().count_rows("users").await.expect("counted");

    fake.sign_in_as(Person::new("subject-of-user2", "User2@sample.com"));
    let outcome = Browser::new(&ts, &fake).sign_in(&org_id).await;

    assert_eq!(outcome, Outcome::Refused("oidc-email-taken".to_owned()));
    assert_eq!(
        ts.app.store().count_rows("users").await.expect("counted"),
        users,
        "no second account"
    );
    ts.teardown().await;
    fake.stop();
}

/// An account that waits comes back through the door it would come in by: an
/// organization that lets no newcomers in, in which it has no place, signs it in no
/// more than it did before.
#[tokio::test]
async fn an_account_that_waits_comes_in_by_no_door_that_was_closed_to_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let browser = Browser::new(&ts, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = ts.client_with_token(browser.sign_in(&org_id).await.token());
    let ada = client.me().await.id;
    ask(&client, "ada", "").await.check_ok();
    // Taken out of the organization past the routes, and its door closed.
    for table in ["workspace_members", "organization_members"] {
        ts.app
            .store()
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{ada}'"))
            .await
            .expect("taken out");
    }
    ts.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &json!({"issuer": fake.issuer, "clientId": CLIENT_ID, "autoJoin": false}),
        )
        .await
        .check_ok();

    let outcome = browser.sign_in(&org_id).await;

    assert_eq!(outcome, Outcome::Refused("oidc-join-closed".to_owned()));
    ts.teardown().await;
    fake.stop();
}

/// An organization that would go with the account, and takes only sessions made
/// through its provider, is not given up by a session made with a password: how much
/// it holds is not told to it, and the asking is refused as everything of the
/// organization is.
#[tokio::test]
async fn an_organization_behind_the_wall_does_not_go_by_a_password() {
    let fake = FakeProvider::start().await;
    let ts = Setup::one_user()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await;
    let org_id = org_of(&ts).await;
    // The instance's own organization, its oldest, goes with no account: its only
    // administrator is held back as the only one of an organization with other
    // people is, alone in it or not.
    let own: Value = ts.client.get(DELETION).await.ok_json();
    let space = gone_space(&own_workspace(&ts.client).await.org_id);
    assert_eq!(own["organizations"], json!([space]), "{own}");
    assert_eq!(
        own["soleAdminOf"],
        json!([{"id": org_id, "name": ORG_NAME}]),
        "{own}"
    );
    if ts.app.store().db_type() != fvoci_store::DbType::Postgres {
        // SQLite holds that one organization only.
        ts.teardown().await;
        fake.stop();
        return;
    }
    // With an older organization on the server, this one is an organization like any
    // other, and has nobody but the account.
    crate::workspaces::another_organization(&ts).await;
    ts.app
        .store()
        .system()
        .execute_unchecked("UPDATE organizations SET created_at = 1 WHERE name = 'another'")
        .await
        .expect("the other organization is the older");
    set_provider(&ts, &org_id, &fake).await;
    let through_provider = admin_through_provider(&ts, &fake, &org_id).await;
    let open: Value = ts.client.get(DELETION).await.ok_json();
    assert_eq!(
        open["organizations"],
        json!([
            space,
            {"id": org_id, "kind": "shared", "name": ORG_NAME, "boards": 0, "wikis": 0, "diaries": 0, "courses": 0, "files": 0}
        ]),
        "{open}"
    );
    require_sso(&through_provider, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();

    let closed: Value = ts.client.get(DELETION).await.ok_json();
    assert_eq!(
        closed["organizations"],
        json!([space, {"id": org_id, "kind": "shared", "name": ORG_NAME}]),
        "nothing of what is in it, and the account's own space as it was: {closed}"
    );
    let refused = ask(&ts.client, "user1", PASSWORD).await;
    refused.check_unauthorized();
    assert_eq!(refused.error_reason(), "sso_required", "{}", refused.text());
    assert_eq!(refused.value()["orgId"], json!(org_id));

    // Through the provider it is the account's to ask; this one is the instance's last
    // administrator, who stays.
    let shown: Value = through_provider.get(DELETION).await.ok_json();
    assert_eq!(shown["organizations"][1]["boards"], json!(0), "{shown}");
    let last = ask(&through_provider, "user1", PASSWORD).await;
    last.check_status(StatusCode::CONFLICT);
    assert_eq!(last.error_reason(), "last_admin");
    ts.teardown().await;
    fake.stop();
}

/// Requesting account deletion reuses the `own_space` already read in the initial
/// standing check when ending public-link readers, instead of reading the full deletion
/// standing a second time.
#[tokio::test]
async fn requesting_account_deletion_does_not_reread_deletion_standing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use fvoci_model::{DeleteAccountRequest, Session};
    use fvoci_store::{Access, Call, Observer};

    #[derive(Debug, Clone, Default)]
    struct ReadCount(Arc<AtomicUsize>);
    impl Observer for ReadCount {
        fn called(&self, call: Call) {
            if call.access == Access::Read {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn waited_for_connection(&self, _: fvoci_store::DbType, _: Duration) {}
    }

    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let reads = ReadCount::default();
    assert!(
        th.app.store().observe(Box::new(reads.clone())),
        "install observer"
    );

    let session = Session {
        user_id: user2.id.clone(),
        ..Session::default()
    };
    let request = DeleteAccountRequest {
        username: USER2_USERNAME.to_owned(),
        password: PASSWORD.to_owned(),
    };
    reads.0.store(0, Ordering::Relaxed);
    th.app
        .request_account_deletion(&session, &request, None)
        .await
        .expect("deletion requested");
    let count = reads.0.load(Ordering::Relaxed);
    // 1: get_user_by_id, 2: account_deletion_standing, 3: shared_container_ids of own space.
    // Re-reading account_deletion_standing inside end_readers_of_own_space would make 4.
    assert_eq!(
        count, 3,
        "request_account_deletion should make 3 reads (not re-read standing), did {count}"
    );
    th.teardown().await;
}
