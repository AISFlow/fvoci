//! An account changes its email address: asked for with the password, made by the
//! link mailed to the new address or, without mail, at once; what the answers tell,
//! what goes with the address, and who may not.

use std::time::Duration;

use fvoci_model::Me;
use fvoci_server::auth::now_millis;
use fvoci_server::auth::secret::token_digest;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{Client, PASSWORD, Setup, TestServer, USER1_USERNAME, USER2_USERNAME};
use crate::email_verification::registration;
use crate::email_verification_more::{settled, switched_on};
use crate::fake_smtp::{FakeSmtp, Received};

const ASK: &str = "/users/me/email-change";
const RESEND: &str = "/users/me/email-change/resend";
const CONFIRM: &str = "/email-change/confirm";
const OLD: &str = "user1@sample.com";
const NEW: &str = "moved@example.test";

async fn mailing(smtp: &FakeSmtp) -> TestServer {
    Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await
}

async fn me(client: &Client) -> Me {
    client.get("/users/me").await.ok_json()
}

/// Asks for the address of the client's account to become `email`.
async fn ask(client: &Client, email: &str) {
    client
        .post(ASK, &json!({"email": email, "password": PASSWORD}))
        .await
        .check_no_content();
}

/// The token of the link in a message, which leads to `page`.
fn token_in(message: &Received, page: &str) -> String {
    let link = message
        .decoded
        .lines()
        .find(|line| line.starts_with("http") && line.contains(page))
        .unwrap_or_else(|| panic!("a link to {page}: {}", message.decoded));
    link.split_once('#')
        .expect("a fragment")
        .1
        .trim()
        .to_owned()
}

/// The messages to `recipient` so far, once the server has nothing more to send.
async fn mailed_to(th: &TestServer, smtp: &FakeSmtp, recipient: &str) -> Vec<Received> {
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the server's background work ends"
    );
    smtp.messages()
        .await
        .into_iter()
        .filter(|message| message.recipient == recipient)
        .collect()
}

async fn confirm(th: &TestServer, token: &str) -> crate::common::ApiResponse {
    th.anonymous().post(CONFIRM, &json!({"token": token})).await
}

/// The link is invalid: 400 naming the token, whatever became of it.
#[track_caller]
fn check_invalid(response: &crate::common::ApiResponse) {
    response.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.refused_at(), "/token", "{}", response.text());
}

/// Asked for with the password, the change waits: the address, the login and the reset
/// mail stay with the old address. A link mailed again ends the one before it, and
/// taking the change back ends them all.
#[tokio::test]
async fn a_change_waits_and_the_account_keeps_its_address() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let user_id = th.client.me().await.id;

    // Not without the password, and not to what is no address or the same one.
    let wrong = th
        .client
        .post(ASK, &json!({"email": NEW, "password": "not the password"}))
        .await;
    wrong.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(wrong.error_reason(), "invalid_credentials");
    assert_eq!(wrong.refused_at(), "/password");
    th.client
        .post(ASK, &json!({"email": NEW}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    for email in ["no address", "USER1@Sample.com", ""] {
        let refused = th
            .client
            .post(ASK, &json!({"email": email, "password": PASSWORD}))
            .await;
        refused.check_status(StatusCode::BAD_REQUEST);
        assert_eq!(refused.refused_at(), "/email", "{email}");
    }
    th.anonymous()
        .post(ASK, &json!({"email": NEW, "password": PASSWORD}))
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(me(&th.client).await.pending_email, None);
    assert!(
        mailed_to(&th, &smtp, NEW).await.is_empty(),
        "a refused asking mails nothing"
    );

    ask(&th.client, &format!("  {NEW} ")).await;
    let waiting = me(&th.client).await;
    assert_eq!(
        (waiting.email.as_str(), waiting.pending_email.as_deref()),
        (OLD, Some(NEW)),
        "the address is the old one until the new one is confirmed"
    );
    let first = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(first.len(), 1, "one link to the new address");
    assert!(
        first[0].decoded.contains(USER1_USERNAME),
        "the account is named: {}",
        first[0].decoded
    );
    let first = token_in(&first[0], "/confirm-email#");
    // Stored as a digest only.
    let store = th.app.store();
    let stands = async |digest: &str| {
        store
            .email_change_link_stands(&user_id, digest, now_millis())
            .await
            .expect("asked")
    };
    assert!(
        stands(&token_digest(&first)).await,
        "the link is known by its digest"
    );
    assert!(!stands(&first).await, "the token itself is not stored");

    // Until then the old address is the account's: login by it, and reset mail to it.
    th.anonymous()
        .post("/login", &json!({"email": NEW, "password": PASSWORD}))
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    for email in [NEW, OLD] {
        th.anonymous()
            .post("/password-reset/request", &json!({"email": email}))
            .await
            .check_no_content();
    }
    assert_eq!(mailed_to(&th, &smtp, NEW).await.len(), 1, "no reset link");
    let to_old = mailed_to(&th, &smtp, OLD).await;
    assert_eq!(to_old.len(), 1, "the reset link goes to the old address");
    token_in(&to_old[0], "/reset-password#");

    // A mail again ends the link before it; so does taking the change back.
    th.client.post(RESEND, &json!({})).await.check_no_content();
    let again = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(again.len(), 2, "the link again");
    let second = token_in(&again[1], "/confirm-email#");
    check_invalid(&confirm(&th, &first).await);
    th.client.delete(ASK).await.check_no_content();
    assert_eq!(me(&th.client).await.pending_email, None);
    check_invalid(&confirm(&th, &second).await);
    th.client
        .post(RESEND, &json!({}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert_eq!(me(&th.client).await.email, OLD);
    th.teardown().await;
    smtp.stop().await;
}

/// The link mailed to the new address changes it, for whoever holds it, once: the
/// session goes on, the reset link mailed to the old address is dead and none goes
/// there again, and the old address is told without a word of the new one, with the
/// link that gives it back.
#[tokio::test]
async fn the_link_changes_the_address_and_ends_what_went_to_the_old_one() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    // A reset link mailed to the address the account has now.
    th.anonymous()
        .post("/password-reset/request", &json!({"email": OLD}))
        .await
        .check_no_content();
    let old_reset = token_in(&mailed_to(&th, &smtp, OLD).await[0], "/reset-password#");

    ask(&th.client, NEW).await;
    let link = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(link.len(), 1, "one link to the new address");
    let link = token_in(&link[0], "/confirm-email#");
    for junk in ["", "no such token"] {
        check_invalid(&confirm(&th, junk).await);
    }
    // By a browser that is not signed in.
    confirm(&th, &link).await.check_no_content();
    check_invalid(&confirm(&th, &link).await);
    let moved = me(&th.client).await;
    assert_eq!(
        (moved.email.as_str(), moved.pending_email),
        (NEW, None),
        "the session goes on, with the new address"
    );

    let reset = th
        .anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": old_reset, "password": "Another-password-1"}),
        )
        .await;
    reset.check_status(StatusCode::BAD_REQUEST);
    for (email, status) in [
        (OLD, StatusCode::UNAUTHORIZED),
        (NEW, StatusCode::NO_CONTENT),
    ] {
        th.anonymous()
            .post("/login", &json!({"email": email, "password": PASSWORD}))
            .await
            .check_status(status);
    }
    let old_mail = mailed_to(&th, &smtp, OLD).await;
    assert_eq!(old_mail.len(), 2, "the reset link, and the notice");
    let notice = &old_mail[1].decoded;
    assert!(notice.contains(USER1_USERNAME), "{notice}");
    // Its one link gives the address back (`email_revert`).
    token_in(&old_mail[1], "/revert-email#");
    for unsaid in ["moved", "example.test", "/confirm-email#"] {
        assert!(!notice.contains(unsaid), "{unsaid} in the notice: {notice}");
    }
    for email in [OLD, NEW] {
        th.anonymous()
            .post("/password-reset/request", &json!({"email": email}))
            .await
            .check_no_content();
    }
    assert_eq!(mailed_to(&th, &smtp, OLD).await.len(), 2, "nothing more");
    let to_new = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(to_new.len(), 2, "the reset link goes to the new address");
    token_in(&to_new[1], "/reset-password#");
    th.teardown().await;
    smtp.stop().await;
}

/// The answer tells nothing of whose an address is: asked to move to another
/// account's, the account is answered and waits as for a free one, and nothing is
/// mailed. Nothing holds an address that waits: somebody else registers with it, and
/// the link then changes nothing.
#[tokio::test]
async fn the_answer_does_not_tell_whether_an_address_is_taken() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let taken = th
        .client
        .post(
            ASK,
            &json!({"email": "User2@sample.com", "password": PASSWORD}),
        )
        .await;
    assert_eq!(
        me(&th.client).await.pending_email.as_deref(),
        Some("User2@sample.com"),
        "it waits, as for a free address"
    );
    let free = th
        .client
        .post(ASK, &json!({"email": NEW, "password": PASSWORD}))
        .await;
    for answer in [&taken, &free] {
        answer.check_no_content();
    }
    assert_eq!(taken.text(), free.text(), "the two answers are one");
    assert_eq!(
        me(&th.client).await.pending_email.as_deref(),
        Some(NEW),
        "the later asking replaced the one before"
    );
    assert!(
        mailed_to(&th, &smtp, "User2@sample.com").await.is_empty()
            && mailed_to(&th, &smtp, "user2@sample.com").await.is_empty(),
        "nothing is mailed to an address that is somebody's"
    );
    assert_eq!(me(&th.client2).await.email, "user2@sample.com");

    // A waiting change holds nothing: the address is registered by somebody else.
    let link = mailed_to(&th, &smtp, NEW).await;
    let [link] = link.as_slice() else {
        panic!("one link to the free address: {link:?}");
    };
    let link = token_in(link, "/confirm-email#");
    th.register_and_login("newcomer", NEW, PASSWORD).await;
    check_invalid(&confirm(&th, &link).await);
    let after = me(&th.client).await;
    assert_eq!(after.email, OLD);
    assert_eq!(
        after.pending_email.as_deref(),
        Some(NEW),
        "the link changed nothing: the change waits as it did"
    );
    assert!(mailed_to(&th, &smtp, OLD).await.is_empty(), "no notice");
    th.teardown().await;
    smtp.stop().await;
}

/// What the account is mailed afterwards tells no more than the answer: a link left
/// out because the address is somebody's, an account's or a registration's that waits,
/// is one of the account's hour as a link mailed is, asked for or asked for again. So
/// the link that would have been the account's third of the hour to its own address
/// is not mailed after two that were left out.
#[tokio::test]
async fn the_mail_that_follows_does_not_tell_whether_an_address_is_taken() {
    let smtp = FakeSmtp::start_on(0).await;
    // A registration beside the server's organization starts another: PostgreSQL's.
    let several = fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres;
    let mut th = mailing(&smtp).await;
    if several {
        fvoci_server::config::ORG_CREATION_OPEN.clone_into(&mut th.config.org_creation);
    }
    let th = switched_on(th, &smtp).await;
    let mut cases = vec![(&th.client, "mine@example.test", "User2@sample.com")];
    if several {
        th.anonymous()
            .post("/register", &registration("kim", "en"))
            .await
            .check_status(StatusCode::ACCEPTED);
        cases.push((&th.client2, "other@example.test", "Kim@example.test"));
    }
    let asking = cases.len();
    for (client, own, taken) in cases {
        ask(client, own).await;
        ask(client, taken).await;
        client.post(RESEND, &json!({})).await.check_no_content();
        assert_eq!(
            me(client).await.pending_email.as_deref(),
            Some(taken),
            "it waits, as for a free address"
        );
        ask(client, own).await;
        assert_eq!(
            mailed_to(&th, &smtp, own).await.len(),
            1,
            "{taken}: the two links left out were the second and third of the hour"
        );
    }
    let links_of_changes = settled(&th, &smtp)
        .await
        .iter()
        .filter(|message| message.decoded.contains("/confirm-email#"))
        .count();
    assert_eq!(
        links_of_changes, asking,
        "one to each account's own address"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// What was asked for under the password before goes with it: a change of the
/// password, a reset by the operator and an asking to delete the account each end the
/// change that waited, and its link.
#[tokio::test]
async fn a_change_that_waits_ends_with_the_password_and_the_account() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let waiting_link = async |client: &Client, password: &str| {
        client
            .post(ASK, &json!({"email": NEW, "password": password}))
            .await
            .check_no_content();
        let mailed = mailed_to(&th, &smtp, NEW).await;
        token_in(mailed.last().expect("a link"), "/confirm-email#")
    };

    let link = waiting_link(&th.client, PASSWORD).await;
    let changed = th
        .client
        .post(
            "/users/me/password",
            &json!({"oldPassword": PASSWORD, "newPassword": "Changed-password-1"}),
        )
        .await;
    changed.check_no_content();
    let client = th.client_with_token(&changed.session_cookie().expect("the new cookie"));
    assert_eq!(me(&client).await.pending_email, None);
    check_invalid(&confirm(&th, &link).await);

    let link = waiting_link(&client, "Changed-password-1").await;
    th.app
        .reset_user_password(USER1_USERNAME, "Operator-password-1")
        .await
        .expect("the operator sets a password");
    check_invalid(&confirm(&th, &link).await);

    // Another account, which administers nothing and so may go.
    let link = waiting_link(&th.client2, PASSWORD).await;
    th.client2
        .post(
            "/users/me/deletion",
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_status(StatusCode::OK);
    check_invalid(&confirm(&th, &link).await);
    // The account that waits to be deleted signs in, and changes nothing of its address.
    let leaving = th
        .anonymous()
        .post(
            "/login",
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await;
    let leaving = th.client_with_token(&leaving.session_cookie().expect("a cookie"));
    let refused = leaving
        .post(ASK, &json!({"email": NEW, "password": PASSWORD}))
        .await;
    refused.check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(refused.error_reason(), "account_deletion_pending");
    assert!(
        mailed_to(&th, &smtp, "user2@sample.com").await.is_empty(),
        "no notice"
    );
    assert!(mailed_to(&th, &smtp, OLD).await.is_empty(), "no notice");
    th.teardown().await;
    smtp.stop().await;
}

/// An account without a password has nothing to confirm the change with, whatever its
/// session: it sets one first.
#[tokio::test]
async fn an_account_without_a_password_sets_one_first() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let user = th
        .app
        .store()
        .get_user_by_username(USER1_USERNAME)
        .await
        .expect("the account");
    th.app
        .store()
        .replace_password_hash(&user.id, &user.password_hash, "")
        .await
        .expect("without a password");
    for password in ["", PASSWORD] {
        let refused = th
            .client
            .post(ASK, &json!({"email": NEW, "password": password}))
            .await;
        refused.check_status(StatusCode::CONFLICT);
        assert_eq!(refused.error_reason(), "password_not_set");
    }
    assert_eq!(me(&th.client).await.pending_email, None);
    assert!(mailed_to(&th, &smtp, NEW).await.is_empty());
    th.teardown().await;
    smtp.stop().await;
}

/// An account, and an address, are mailed no more links an hour than the server's
/// number: more requests are answered the same and mail nothing.
#[tokio::test]
async fn the_links_are_limited_by_account_and_by_address() {
    let smtp = FakeSmtp::start_on(0).await;
    let mut settings = smtp.settings();
    settings.mail_reset_per_account_hour = 2;
    let th = Setup::native()
        .smtp(settings)
        .start()
        .await
        .init_basic()
        .await;
    for email in ["one@example.test", "two@example.test", "three@example.test"] {
        ask(&th.client, email).await;
    }
    for mailed in ["one@example.test", "two@example.test"] {
        assert_eq!(mailed_to(&th, &smtp, mailed).await.len(), 1, "{mailed}");
    }
    assert!(
        mailed_to(&th, &smtp, "three@example.test").await.is_empty(),
        "the account's third link of the hour is not mailed"
    );
    assert_eq!(
        me(&th.client).await.pending_email.as_deref(),
        Some("three@example.test")
    );
    // Another account, to an address that has had one link: one more, and no third.
    for _ in 0..2 {
        ask(&th.client2, "Two@example.test").await;
    }
    assert_eq!(
        mailed_to(&th, &smtp, "Two@example.test").await.len()
            + mailed_to(&th, &smtp, "two@example.test").await.len(),
        2,
        "the address's third link of the hour is not mailed"
    );
    // Which was one of the hour of the account that asked all the same: what it is
    // mailed next does not tell that the address had its number.
    ask(&th.client2, "four@example.test").await;
    assert!(
        mailed_to(&th, &smtp, "four@example.test").await.is_empty(),
        "the account's third asking of the hour is not mailed"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// Nor does it tell through another account: an address that has had its number of
/// links, asked for by somebody else, is one of the asker's hour as any other is.
#[tokio::test]
async fn an_asking_for_an_address_that_has_had_its_links_counts_as_any_other() {
    const FULL: &str = "full@example.test";
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    for _ in 0..3 {
        ask(&th.client2, FULL).await;
    }
    assert_eq!(mailed_to(&th, &smtp, FULL).await.len(), 3);
    for email in [
        "mine@example.test",
        FULL,
        "mine@example.test",
        "mine@example.test",
    ] {
        ask(&th.client, email).await;
    }
    assert_eq!(mailed_to(&th, &smtp, FULL).await.len(), 3, "no fourth");
    assert_eq!(
        mailed_to(&th, &smtp, "mine@example.test").await.len(),
        2,
        "the asking that was not mailed was the second of the account's hour"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// A server that sends no mail confirms no address, as at registration: the password
/// alone changes it, at once, and there an address that is somebody's is said to be.
#[tokio::test]
async fn a_server_without_mail_changes_the_address_with_the_password() {
    let th = Setup::native().start().await.init_basic().await;
    let wrong = th
        .client
        .post(ASK, &json!({"email": NEW, "password": "not the password"}))
        .await;
    wrong.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(wrong.error_reason(), "invalid_credentials");
    let taken = th
        .client
        .post(
            ASK,
            &json!({"email": "USER2@sample.com", "password": PASSWORD}),
        )
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "email_taken");
    assert_eq!(me(&th.client).await.email, OLD);

    ask(&th.client, NEW).await;
    let moved = me(&th.client).await;
    assert_eq!((moved.email.as_str(), moved.pending_email), (NEW, None));
    for (email, status) in [
        (OLD, StatusCode::UNAUTHORIZED),
        (NEW, StatusCode::NO_CONTENT),
    ] {
        th.anonymous()
            .post("/login", &json!({"email": email, "password": PASSWORD}))
            .await
            .check_status(status);
    }
    th.client
        .post(RESEND, &json!({}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client.delete(ASK).await.check_no_content();
    check_invalid(&confirm(&th, "no such token").await);
    th.teardown().await;
}
