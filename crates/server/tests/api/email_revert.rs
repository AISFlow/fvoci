//! The link mailed to the address an account had, with the notice that it changed: it
//! gives that address back, once, to whoever holds it, and ends every session of the
//! account, locks its password until a reset sets a new one, and keeps the address
//! from anybody else while it stands.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, PASSWORD, Setup, TestServer, USER1_USERNAME, USER2_USERNAME,
};
use crate::email_verification_more::switched_on;
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Person};
use crate::fake_smtp::{FakeSmtp, Received};
use crate::oidc::{Browser, Outcome, org_of, set_provider, with_key_and_private_addresses};
use crate::oidc_link::{Ended, Linker};
use crate::websocket::{Event, PATIENCE, Socket};

const ASK: &str = "/users/me/email-change";
const REVERT: &str = "/email-change/revert";
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

/// The token of the last link to `page` mailed to `recipient`.
async fn last_link(th: &TestServer, smtp: &FakeSmtp, recipient: &str, page: &str) -> String {
    let mailed = mailed_to(th, smtp, recipient).await;
    let link = mailed
        .iter()
        .rev()
        .find_map(|message| {
            message
                .decoded
                .lines()
                .find(|line| line.starts_with("http") && line.contains(page))
        })
        .unwrap_or_else(|| panic!("a link to {page} mailed to {recipient}: {mailed:?}"));
    link.split_once('#')
        .expect("a fragment")
        .1
        .trim()
        .to_owned()
}

/// Moves the client's account to `email` by the link mailed there, and returns the
/// token of the link mailed to the address before, which gives it back.
async fn moved(th: &TestServer, smtp: &FakeSmtp, client: &Client, email: &str) -> String {
    moved_with(th, smtp, client, email, PASSWORD).await
}

async fn moved_with(
    th: &TestServer,
    smtp: &FakeSmtp,
    client: &Client,
    email: &str,
    password: &str,
) -> String {
    let before = confirmed(th, smtp, client, email, password).await;
    last_link(th, smtp, &before, "/revert-email#").await
}

/// Moves the client's account to `email` by the link mailed there, and returns the
/// address it had before.
async fn confirmed(
    th: &TestServer,
    smtp: &FakeSmtp,
    client: &Client,
    email: &str,
    password: &str,
) -> String {
    let before: Value = client.get("/users/me").await.ok_json();
    let before = before["email"].as_str().expect("an address").to_owned();
    client
        .post(ASK, &json!({"email": email, "password": password}))
        .await
        .check_no_content();
    let link = last_link(th, smtp, email, "/confirm-email#").await;
    th.anonymous()
        .post("/email-change/confirm", &json!({"token": link}))
        .await
        .check_no_content();
    before
}

async fn revert(th: &TestServer, token: &str) -> ApiResponse {
    th.anonymous().post(REVERT, &json!({"token": token})).await
}

/// The address a link gave back.
#[track_caller]
fn restored(response: &ApiResponse) -> String {
    response.check_ok();
    response.value()["email"]
        .as_str()
        .unwrap_or_else(|| panic!("the address given back: {}", response.text()))
        .to_owned()
}

/// The link is invalid: 400 naming the token, whatever became of it.
#[track_caller]
fn check_invalid(response: &ApiResponse) {
    response.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(response.refused_at(), "/token", "{}", response.text());
}

/// Whether the server closes the connection, after what was on its way to it.
async fn closed(socket: &mut Socket) -> bool {
    loop {
        match socket.next(PATIENCE).await {
            Event::Closed => return true,
            Event::Nothing => return false,
            Event::Json(_) | Event::Ping => {}
        }
    }
}

/// The notice to the address before carries the link, which tells nothing of the new
/// address. Used, by a browser that is not signed in, it gives that address back and
/// ends every session and connection of the account and the reset links; the password
/// signs in no more, as a wrong one does, until a reset sets a new one. It is used once,
/// and a link that is used, unknown or nothing is the same 400. The address it was used
/// at is told, without the one it is now.
#[tokio::test]
async fn the_link_gives_the_address_back_and_ends_every_session() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let elsewhere = th.login(USER1_USERNAME, PASSWORD).await;
    let agent = crate::api_tokens::agent_reading(&th, &th.client).await;
    let mut socket = Socket::connect_as(&th.client).await;
    let back = moved(&th, &smtp, &th.client, NEW).await;
    let notice = mailed_to(&th, &smtp, OLD).await;
    let notice = &notice.last().expect("the notice").decoded;
    assert!(notice.contains(USER1_USERNAME), "{notice}");
    assert!(!notice.contains("moved"), "the new address: {notice}");
    th.anonymous()
        .post("/password-reset/request", &json!({"email": NEW}))
        .await
        .check_no_content();
    let reset = last_link(&th, &smtp, NEW, "/reset-password#").await;

    let unknown = revert(&th, "no such link").await;
    check_invalid(&unknown);
    check_invalid(&revert(&th, "").await);
    assert_eq!(restored(&revert(&th, &back).await), OLD);
    let again = revert(&th, &back).await;
    check_invalid(&again);
    assert_eq!(
        again.problem(),
        unknown.problem(),
        "which it was is not said"
    );

    for client in [&th.client, &elsewhere] {
        client.get("/users/me").await.check_unauthorized();
    }
    crate::api_tokens::check_token_ended(&agent).await;
    assert!(closed(&mut socket).await, "the connection is closed");
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": reset, "password": "Another-password-1"}),
        )
        .await
        .check_status(StatusCode::BAD_REQUEST);
    let locked = th.anonymous().login(USER1_USERNAME, PASSWORD).await;
    let wrong = th
        .anonymous()
        .login(USER1_USERNAME, "not the password")
        .await;
    locked.check_unauthorized();
    assert_eq!(
        locked.problem(),
        wrong.problem(),
        "refused as a wrong password is"
    );
    th.anonymous()
        .post("/login", &json!({"email": OLD, "password": PASSWORD}))
        .await
        .check_unauthorized();

    let told = mailed_to(&th, &smtp, NEW).await;
    let told = &told
        .last()
        .expect("the notice of the address given back")
        .decoded;
    assert!(told.contains(USER1_USERNAME), "{told}");
    for unsaid in ["sample.com", "user1@", "http"] {
        assert!(!told.contains(unsaid), "{unsaid} in the notice: {told}");
    }

    // A reset to the address given back sets a password that signs in.
    th.anonymous()
        .post("/password-reset/request", &json!({"email": OLD}))
        .await
        .check_no_content();
    let reset = last_link(&th, &smtp, OLD, "/reset-password#").await;
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": reset, "password": "Recovered-password-1"}),
        )
        .await
        .check_no_content();
    let back_in = th.login(USER1_USERNAME, "Recovered-password-1").await;
    let me: Value = back_in.get("/users/me").await.ok_json();
    assert_eq!(me["email"], OLD, "{me}");
    th.teardown().await;
    smtp.stop().await;
}

/// The links of A to B and of B to C: whoever used the later one, to have B again, did
/// not end the earlier one, which gives A back after it. Used first, the earlier one
/// ends the later. Nothing the account does in between ends either: a new password, a
/// change asked for and taken back, another one that waits.
#[tokio::test]
async fn a_later_link_does_not_end_an_earlier_one() {
    let smtp = FakeSmtp::start_on(0).await;
    // Six askings of one account in an hour.
    let mut settings = smtp.settings();
    settings.mail_reset_per_account_hour = 6;
    let th = Setup::native()
        .smtp(settings)
        .start()
        .await
        .init_basic()
        .await;
    let to_a = moved(&th, &smtp, &th.client, "b@example.test").await;
    let to_b = moved(&th, &smtp, &th.client, "c@example.test").await;
    let changed = th
        .client
        .post(
            "/users/me/password",
            &json!({"oldPassword": PASSWORD, "newPassword": "Changed-password-1"}),
        )
        .await;
    changed.check_no_content();
    let client = th.client_with_token(&changed.session_cookie().expect("the new cookie"));
    for email in ["d@example.test", "e@example.test"] {
        client
            .post(
                ASK,
                &json!({"email": email, "password": "Changed-password-1"}),
            )
            .await
            .check_no_content();
    }
    client.delete(ASK).await.check_no_content();
    client
        .post(
            ASK,
            &json!({"email": "f@example.test", "password": "Changed-password-1"}),
        )
        .await
        .check_no_content();

    assert_eq!(restored(&revert(&th, &to_b).await), "b@example.test");
    assert_eq!(restored(&revert(&th, &to_a).await), OLD);
    // Each told at the address it was used at, by a notice that leads nowhere.
    for recipient in ["c@example.test", "b@example.test"] {
        let mailed = mailed_to(&th, &smtp, recipient).await;
        let last = &mailed.last().expect("mailed").decoded;
        assert!(!last.contains("http"), "{recipient}: {last}");
    }

    // The other way round, by another account.
    let to_a = moved(&th, &smtp, &th.client2, "b2@example.test").await;
    let to_b = moved(&th, &smtp, &th.client2, "c2@example.test").await;
    assert_eq!(restored(&revert(&th, &to_a).await), "user2@sample.com");
    check_invalid(&revert(&th, &to_b).await);
    th.teardown().await;
    smtp.stop().await;
}

/// While its link stands, the address the account had is nobody else's: another
/// account's change to it is answered as any and mails nothing, and a registration or a
/// link for members is refused as for an address an account has. The account's own
/// change back to it is mailed and made.
#[tokio::test]
async fn while_a_link_stands_nobody_else_takes_the_address() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    moved(&th, &smtp, &th.client, NEW).await;
    let mailed_before = mailed_to(&th, &smtp, OLD).await.len();

    th.client2
        .post(
            ASK,
            &json!({"email": "User1@Sample.com", "password": PASSWORD}),
        )
        .await
        .check_no_content();
    let me: Value = th.client2.get("/users/me").await.ok_json();
    assert_eq!(me["pendingEmail"], "User1@Sample.com", "it waits: {me}");
    assert_eq!(
        mailed_to(&th, &smtp, OLD).await.len()
            + mailed_to(&th, &smtp, "User1@Sample.com").await.len(),
        mailed_before,
        "nothing is mailed to it"
    );

    let link = th.member_link().await;
    let accept = async |email: &str| {
        th.anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": link, "account": {
                    "username": "newcomer", "email": email, "password": PASSWORD,
                }}),
            )
            .await
    };
    let held = accept(OLD).await;
    let taken = accept("user2@sample.com").await;
    held.check_status(StatusCode::CONFLICT);
    assert_eq!(held.error_reason(), "email_taken");
    assert_eq!(held.problem(), taken.problem(), "as an account's");

    // The account itself may have it again by a change, mailed as any.
    let back = moved(&th, &smtp, &th.client, "User1@Sample.com").await;
    let me: Value = th.client.get("/users/me").await.ok_json();
    assert_eq!(me["email"], "User1@Sample.com", "{me}");
    assert_eq!(restored(&revert(&th, &back).await), NEW);
    th.teardown().await;
    smtp.stop().await;
}

/// The notice that carries a link is not one of the account's hour of notices: told
/// three times in the hour, of two changes and of an address given back, the account
/// moved again still has the link mailed to the address it had.
#[tokio::test]
async fn the_notice_with_a_link_is_not_left_out_for_the_notices_of_the_hour() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    moved(&th, &smtp, &th.client, "b@example.test").await;
    let to_b = moved(&th, &smtp, &th.client, "c@example.test").await;
    assert_eq!(restored(&revert(&th, &to_b).await), "b@example.test");
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "b@example.test"}),
        )
        .await
        .check_no_content();
    let reset = last_link(&th, &smtp, "b@example.test", "/reset-password#").await;
    let password = "Recovered-password-1";
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": reset, "password": password}),
        )
        .await
        .check_no_content();
    let client = th.login(USER1_USERNAME, password).await;
    let before = mailed_to(&th, &smtp, "b@example.test").await.len();

    confirmed(&th, &smtp, &client, "d@example.test", password).await;
    let mailed = mailed_to(&th, &smtp, "b@example.test").await;
    assert_eq!(mailed.len(), before + 1, "the notice is mailed");
    let notice = &mailed.last().expect("the notice").decoded;
    assert!(notice.contains("/revert-email#"), "with its link: {notice}");
    th.teardown().await;
    smtp.stop().await;
}

/// While the recovery queue has no place for the notice that carries the link back, a
/// change is not made: 503, and the link changes the address once there is one.
#[tokio::test]
async fn no_change_is_made_while_its_notice_has_no_place() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| config.smtp.mail_reset_queue = 1)
        .start()
        .await
        .init_basic()
        .await;
    th.client
        .post(ASK, &json!({"email": NEW, "password": PASSWORD}))
        .await
        .check_no_content();
    let link = last_link(&th, &smtp, NEW, "/confirm-email#").await;
    // A reset link held on its way keeps the queue's one place.
    smtp.hold("@sample.com");
    let attempts = smtp.attempts();
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "user2@sample.com"}),
        )
        .await
        .check_no_content();
    smtp.attempted(attempts + 1).await;

    let confirm = async || {
        th.anonymous()
            .post("/email-change/confirm", &json!({"token": link}))
            .await
    };
    confirm()
        .await
        .check_status(StatusCode::SERVICE_UNAVAILABLE);
    let me: Value = th.client.get("/users/me").await.ok_json();
    assert_eq!(me["email"], OLD, "not changed: {me}");
    assert_eq!(me["pendingEmail"], NEW, "it still waits: {me}");

    smtp.release();
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    confirm().await.check_no_content();
    let back = last_link(&th, &smtp, OLD, "/revert-email#").await;
    assert_eq!(restored(&revert(&th, &back).await), OLD);
    th.teardown().await;
    smtp.stop().await;
}

/// An account made through a provider sets a password, moves its address under it, and
/// has it given back: the provider still signs it in, and to that session the password
/// confirms nothing and changes nothing, as a wrong one would not.
#[tokio::test]
async fn a_locked_password_is_refused_as_a_wrong_one_and_the_provider_still_signs_in() {
    let smtp = FakeSmtp::start_on(0).await;
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    set_provider(&th, &org_id, &fake).await;
    let th = switched_on(th, &smtp).await;
    let browser = Browser::new(&th, &fake);
    let mut person = Person::new("subject-ada", "ada@example.com");
    person.preferred_username = Some("ada".to_owned());
    fake.sign_in_as(person);
    let client = th.client_with_token(browser.sign_in(&org_id).await.token());
    let set = client
        .put("/users/me/password", &json!({"password": "Ada-password-1"}))
        .await;
    set.check_no_content();
    let client = th.client_with_token(&set.session_cookie().expect("the new cookie"));
    // The provider's address is the identity's: the account has its own from here,
    // and a link back to it from the next.
    let first = confirmed(&th, &smtp, &client, "ada@example.org", "Ada-password-1").await;
    assert_eq!(first, "", "no address of its own");
    let back = moved_with(
        &th,
        &smtp,
        &client,
        "ada.moved@example.org",
        "Ada-password-1",
    )
    .await;
    assert_eq!(restored(&revert(&th, &back).await), "ada@example.org");
    client.get("/users/me").await.check_unauthorized();

    th.anonymous()
        .login("ada", "Ada-password-1")
        .await
        .check_unauthorized();
    let client = th.client_with_token(browser.sign_in(&org_id).await.token());
    let identities: Value = client.get("/users/me/identities").await.ok_json();
    assert_eq!(identities.as_array().map(Vec::len), Some(1), "{identities}");
    // A wrong password of another account, whose failed attempts are counted apart.
    let same = async |path: &str, body: &dyn Fn(&str) -> Value| {
        let locked = client.post(path, &body("Ada-password-1")).await;
        let wrong = th.client.post(path, &body("not the password")).await;
        assert_eq!(locked.problem(), wrong.problem(), "{path}");
        assert_ne!(locked.status, StatusCode::NO_CONTENT, "{path}");
    };
    same(
        "/users/me/password/confirm",
        &|password| json!({"password": password}),
    )
    .await;
    same(
        "/users/me/password",
        &|password| json!({"oldPassword": password, "newPassword": "Taken-over-1"}),
    )
    .await;
    same(
        ASK,
        &|password| json!({"email": "elsewhere@example.org", "password": password}),
    )
    .await;
    th.teardown().await;
    smtp.stop().await;
    fake.stop();
}

/// A link left out because its address is held takes the turn of the pace that one
/// mailed would: the next message to anybody waits for it as it would after one that
/// left, so how soon it comes tells nothing of the address. Held by an account, and by
/// a link that gives it back.
#[tokio::test]
async fn a_link_left_out_takes_its_turn_of_the_pace() {
    // One message a second: the second waits a second for its turn.
    const INTERVAL: Duration = Duration::from_secs(1);
    for held_by_a_link in [false, true] {
        let smtp = FakeSmtp::start_on(0).await;
        let mut settings = smtp.settings();
        settings.mail_per_second = 1;
        let th = Setup::native()
            .smtp(settings)
            .start()
            .await
            .init_basic()
            .await;
        let held = if held_by_a_link {
            // Moved past the mailer, which would take turns of the pace.
            let store = th.app.store();
            let user2 = store
                .get_user_by_username(USER2_USERNAME)
                .await
                .expect("the account");
            let now = fvoci_server::auth::now_millis();
            assert!(
                store
                    .ask_email_change(&user2.id, &user2.password_hash, "two@example.test", now)
                    .await
                    .expect("asked")
            );
            assert!(
                store
                    .renew_email_change_link(&user2.id, "two@example.test", "link", now)
                    .await
                    .expect("a link")
            );
            store
                .confirm_email_change("link", "back", now)
                .await
                .expect("confirmed")
                .expect("moved");
            "User2@Sample.com"
        } else {
            "user2@sample.com"
        };
        tokio::time::sleep(INTERVAL).await;
        th.client
            .post(ASK, &json!({"email": held, "password": PASSWORD}))
            .await
            .check_no_content();
        assert!(th.app.finish_background(Duration::from_secs(10)).await);
        let asked = Instant::now();
        th.client
            .post(ASK, &json!({"email": NEW, "password": PASSWORD}))
            .await
            .check_no_content();
        assert_eq!(mailed_to(&th, &smtp, NEW).await.len(), 1);
        let waited = asked.elapsed();
        assert!(
            waited >= INTERVAL.mul_f32(0.8),
            "held by a link: {held_by_a_link}; the link came after {waited:?}"
        );
        assert!(
            smtp.messages()
                .await
                .iter()
                .all(|message| { !message.recipient.eq_ignore_ascii_case("user2@sample.com") })
        );
        th.teardown().await;
        smtp.stop().await;
    }
}

/// An SSO identity linked before the email change stays after reverting, while one an
/// attacker linked after the change is removed and can no longer sign in to the
/// account.
#[tokio::test]
async fn an_identity_linked_after_the_change_is_removed_on_revert_and_an_earlier_one_stays() {
    let smtp = FakeSmtp::start_on(0).await;
    let fake = FakeProvider::start().await;
    let th = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    // Disable auto-join so unlinked subjects are refused rather than creating a new account.
    th.client
        .put(
            &format!("/org/{org_id}/oidc"),
            &json!({
                "issuer": fake.issuer,
                "clientId": CLIENT_ID,
                "clientSecret": CLIENT_SECRET,
                "autoJoin": false,
            }),
        )
        .await
        .check_ok();
    let th = switched_on(th, &smtp).await;
    let user1_id = th.client.me().await.id;
    let linker = Linker::new(&th, &fake);
    let browser = Browser::new(&th, &fake);

    // Owner links their original SSO identity before any email change.
    fake.sign_in_as(Person::new("subject-owner", OLD));
    assert_eq!(linker.link(&org_id, &th.client.token).await, Ended::Linked);

    // Attacker changes email and links their own SSO identity after the change.
    let back = moved(&th, &smtp, &th.client, NEW).await;
    fake.sign_in_as(Person::new("subject-attacker", NEW));
    assert_eq!(linker.link(&org_id, &th.client.token).await, Ended::Linked);
    let attacker_session = th.client_with_token(browser.sign_in(&org_id).await.token());
    assert_eq!(attacker_session.me().await.id, user1_id);
    let before: Value = th.client.get("/users/me/identities").await.ok_json();
    assert_eq!(before.as_array().map(Vec::len), Some(2), "{before}");

    // Owner uses the revert link: attacker's session and SSO identity both end.
    assert_eq!(restored(&revert(&th, &back).await), OLD);
    attacker_session.get("/users/me").await.check_unauthorized();

    fake.sign_in_as(Person::new("subject-attacker", NEW));
    assert!(
        matches!(browser.sign_in(&org_id).await, Outcome::Refused(_)),
        "the attacker's SSO identity can no longer sign in"
    );

    // Owner's original SSO identity still signs in to the restored account.
    fake.sign_in_as(Person::new("subject-owner", OLD));
    let owner_session = th.client_with_token(browser.sign_in(&org_id).await.token());
    let me: Value = owner_session.get("/users/me").await.ok_json();
    assert_eq!(me["id"], user1_id);
    assert_eq!(me["email"], OLD);
    let after: Value = owner_session.get("/users/me/identities").await.ok_json();
    let subjects: Vec<&str> = after
        .as_array()
        .expect("identities")
        .iter()
        .filter_map(|entry| entry["subject"].as_str())
        .collect();
    assert_eq!(subjects, ["subject-owner"]);

    th.teardown().await;
    smtp.stop().await;
    fake.stop();
}

/// When the old address is already held by another account, reverting returns `200 {}`
/// and clears the account's email address so the address set after the change cannot
/// receive password reset mail; an operator reset link still recovers the account.
#[tokio::test]
async fn when_the_old_address_is_held_revert_clears_email_and_blocks_reset() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = mailing(&smtp).await;
    let user1_id = th.client.me().await.id;
    let user2_id = th.client2.me().await.id;
    let back = moved(&th, &smtp, &th.client, NEW).await;
    // Also queue a pending change to verify it is discarded too.
    th.client
        .post(
            ASK,
            &json!({"email": "third@example.test", "password": PASSWORD}),
        )
        .await
        .check_no_content();
    assert!(th.app.finish_background(Duration::from_secs(10)).await);
    let before_revert_mail = mailed_to(&th, &smtp, NEW).await.len();
    // Another account holds the old address by now.
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET email = '{OLD}' WHERE id = '{user2_id}'"
        ))
        .await
        .expect("planted");

    let answer = revert(&th, &back).await;
    answer.check_ok();
    assert_eq!(answer.value(), json!({}), "200 {{}}: {}", answer.text());
    let stored = th
        .app
        .store()
        .get_user_by_id(&user1_id)
        .await
        .expect("account");
    assert_eq!(stored.email, "", "email cleared");
    assert!(
        stored.password_is_locked(),
        "password must be locked after revert"
    );
    assert_eq!(
        th.app
            .store()
            .email_change_of(&user1_id, fvoci_server::auth::now_millis())
            .await
            .expect("read"),
        None
    );

    // The previous address is notified that it is no longer the account's (without a link).
    let after_revert_mail = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(
        after_revert_mail.len(),
        before_revert_mail + 1,
        "revert notice mailed to {NEW}: {after_revert_mail:?}"
    );
    let notice = &after_revert_mail.last().expect("the revert notice").decoded;
    assert!(notice.contains(USER1_USERNAME), "{notice}");
    assert!(!notice.contains("http"), "no link in notice: {notice}");

    // The attacker's address receives no password reset link.
    th.anonymous()
        .post("/password-reset/request", &json!({"email": NEW}))
        .await
        .check_no_content();
    let to_new = mailed_to(&th, &smtp, NEW).await;
    assert_eq!(
        to_new.len(),
        after_revert_mail.len(),
        "no additional mail sent to {NEW}: {to_new:?}"
    );
    assert!(
        to_new
            .iter()
            .all(|mail| !mail.decoded.contains("/reset-password#")),
        "no password reset link is mailed to {NEW}: {to_new:?}"
    );
    th.anonymous()
        .post("/login", &json!({"email": NEW, "password": PASSWORD}))
        .await
        .check_unauthorized();

    // Operator recovery by username still works and shows an empty email on /users/me.
    let url = th
        .app
        .password_reset_link(USER1_USERNAME)
        .await
        .expect("operator reset link");
    let token = url.split_once('#').expect("fragment").1.trim();
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "Recovered-by-op-1"}),
        )
        .await
        .check_no_content();
    let recovered = th.login(USER1_USERNAME, "Recovered-by-op-1").await;
    let me: Value = recovered.get("/users/me").await.ok_json();
    assert_eq!(me["email"], "");
    assert!(me["pendingEmail"].is_null(), "pendingEmail cleared: {me}");

    th.teardown().await;
    smtp.stop().await;
}
