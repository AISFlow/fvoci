//! Registrations that wait for their address, after the first link: a new link, the
//! hourly limit, the seven days, and the accounts whose addresses were proven another
//! way, which wait for nothing.

use std::time::Duration;

use fvoci_model::Invitation;
use fvoci_store::{SIGNUP_LIFETIME_MILLIS, SignupLookup};
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, USER1_USERNAME};
use crate::email_verification::{
    COMPLETE, SIGNUP_PASSWORD, complete_elsewhere, complete_with_marker, link_token, marker_of,
    registration, verifying,
};
use crate::fake_oidc::{FakeProvider, Person};
use crate::fake_smtp::{FakeSmtp, Received};
use crate::oidc::{Browser, org_of, set_provider, with_key_and_private_addresses};

const RESEND: &str = "/email-verification/request";

/// The same server and database, started again with registrations confirmed by mail
/// sent through `smtp`: as an operator turns the switch on.
pub(crate) async fn switched_on(mut th: TestServer, smtp: &FakeSmtp) -> TestServer {
    th.config.smtp = smtp.settings();
    th.config.mail.email_verification = true;
    th.killed_and_started_again(async |_| {}).await
}

/// The messages `smtp` holds once the background work is done.
pub(crate) async fn settled(th: &TestServer, smtp: &FakeSmtp) -> Vec<Received> {
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the mail is done with"
    );
    smtp.messages().await
}

#[tokio::test]
async fn a_new_link_ends_the_one_before_and_an_address_gets_three_an_hour() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let registered = th
        .anonymous()
        .post("/register", &registration("kim", "en"))
        .await;
    registered.check_status(StatusCode::ACCEPTED);
    let marker = marker_of(&registered).expect("the marker of the browser that registered");
    let first = link_token(&smtp.received(1).await[0]);

    th.anonymous()
        .post(RESEND, &json!({"username": "KIM", "language": "ko"}))
        .await
        .check_no_content();
    let messages = smtp.received(2).await;
    assert!(
        messages[1].decoded.contains("lang=\"en\""),
        "the registration's own language first"
    );
    let second = link_token(&messages[1]);
    assert_ne!(first, second, "a link of its own");
    let ended = th
        .anonymous()
        .post(COMPLETE, &json!({"token": first}))
        .await;
    ended.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(ended.refused_at(), "/token", "the first link ended");

    // By the address; then once more than the hour takes, answered the same.
    for _ in 0..2 {
        th.anonymous()
            .post(RESEND, &json!({"email": "Kim@Example.test"}))
            .await
            .check_no_content();
    }
    // Nobody's name, and nothing named: the one answer, and a refusal of the form.
    th.anonymous()
        .post(RESEND, &json!({"username": "nobody"}))
        .await
        .check_no_content();
    th.anonymous()
        .post(RESEND, &json!({}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    let messages = settled(&th, &smtp).await;
    assert_eq!(messages.len(), 3, "three an hour, the first counted");
    let third = link_token(&messages[2]);
    th.anonymous()
        .post(COMPLETE, &json!({"token": second}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    // The browser is the registration's, whichever of its links it opens.
    let confirmed = complete_with_marker(&th, &third, &marker).await;
    confirmed.check_status(StatusCode::OK);
    assert!(confirmed.session_cookie().is_some(), "signed in");

    // Confirmed, it is an account: no link is sent for it any more.
    th.anonymous()
        .post(RESEND, &json!({"username": "kim"}))
        .await
        .check_no_content();
    assert_eq!(settled(&th, &smtp).await.len(), 3, "nothing for an account");
    th.teardown().await;
    smtp.stop().await;
}

#[tokio::test]
async fn a_registration_not_confirmed_in_seven_days_goes_and_frees_its_names() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    th.anonymous()
        .post("/register", &registration("lena", "en"))
        .await
        .check_status(StatusCode::ACCEPTED);
    let token = link_token(&smtp.received(1).await[0]);
    let housekeeping = th.app.housekeeping();
    let now = fvoci_server::auth::now_millis();
    assert_eq!(
        housekeeping
            .remove_ended_signups(now)
            .await
            .expect("cleaned"),
        0,
        "not before its seven days"
    );
    let later = now + SIGNUP_LIFETIME_MILLIS + 1;
    assert_eq!(
        housekeeping
            .remove_ended_signups(later)
            .await
            .expect("cleaned"),
        1,
        "gone after them"
    );
    assert!(
        th.app
            .store()
            .live_signup(SignupLookup::Username("lena"), now)
            .await
            .expect("read")
            .is_none(),
        "no trace of it"
    );
    th.anonymous()
        .post(COMPLETE, &json!({"token": token}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.anonymous()
        .login("lena", "Pa$$word-1")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    // The name and the address are free again.
    th.anonymous()
        .post("/register", &registration("lena", "en"))
        .await
        .check_status(StatusCode::ACCEPTED);
    th.teardown().await;
    smtp.stop().await;
}

/// A registration made while the switch was on still gets its link, and is confirmed,
/// once the switch is off; a server that sends no mail has no links to send.
#[tokio::test]
async fn a_registration_from_before_the_switch_went_off_still_gets_its_link() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    th.anonymous()
        .post("/register", &registration("kai", "en"))
        .await
        .check_status(StatusCode::ACCEPTED);
    smtp.received(1).await;
    let mut th = th;
    th.config.mail.email_verification = false;
    let th = th.killed_and_started_again(async |_| {}).await;
    let refused = th.anonymous().login("kai", "Pa$$word-1").await;
    assert_eq!(
        refused.error_reason(),
        "email_unverified",
        "still no account"
    );
    th.anonymous()
        .post(RESEND, &json!({"username": "kai"}))
        .await
        .check_no_content();
    let token = link_token(&smtp.received(2).await[1]);
    complete_elsewhere(&th, &token, SIGNUP_PASSWORD)
        .await
        .check_status(StatusCode::OK);
    th.teardown().await;

    let without_mail = Setup::native().start().await;
    without_mail
        .anonymous()
        .post(RESEND, &json!({"username": "kai"}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    without_mail.teardown().await;
    smtp.stop().await;
}

/// The hourly clean-up removes what ended: a registration past its seven days goes.
#[tokio::test]
async fn the_hourly_clean_up_removes_the_registrations_that_ended() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    let long_ago = fvoci_server::auth::now_millis() - SIGNUP_LIFETIME_MILLIS - 1;
    let ended = fvoci_store::Signup {
        id: fvoci_model::new_id(),
        username: "old".to_owned(),
        email: "old@example.test".to_owned(),
        custom_name: String::new(),
        password_hash: "hash".to_owned(),
        language: String::new(),
        start: fvoci_store::SignupStart::Organization("Old".to_owned()),
        browser_hash: String::new(),
        created_at: long_ago - 1,
        expires_at: long_ago,
    };
    let store = th.app.store();
    store
        .create_signup(&ended, None, long_ago - 1)
        .await
        .expect("stored as it was");
    // Not counted here: the clean-up the server runs as it starts may have come first.
    fvoci_server::keep_house(&th.app).await;
    assert_eq!(
        store.count_rows("signups").await.expect("counted"),
        0,
        "removed by the clean-up"
    );
    th.teardown().await;
    smtp.stop().await;
}

/// Accounts made before the switch was turned on, and accounts a provider vouches for,
/// sign in as they did; a reset link proves the address of an account it is used on.
#[tokio::test]
async fn accounts_from_before_and_through_a_provider_need_no_confirmation() {
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
    assert_eq!(
        th.anonymous().get("/login-options").await.value()["emailVerification"],
        true,
        "the switch is on"
    );

    th.anonymous()
        .login(USER1_USERNAME, PASSWORD)
        .await
        .check_no_content();
    th.client.me().await;

    let mut person = Person::new("subject-mo", "mo@example.com");
    person.preferred_username = Some("mo".to_owned());
    fake.sign_in_as(person);
    let signed_in = Browser::new(&th, &fake).sign_in(&org_id).await;
    let me = th.client_with_token(signed_in.token()).me().await;
    assert_eq!(me.username, "mo", "made at the first sign-in");

    // A reset link mailed to the address sets the password, and the account signs in.
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "user1@sample.com"}),
        )
        .await
        .check_no_content();
    let reset = smtp.received(1).await;
    let link = reset[0]
        .decoded
        .lines()
        .find(|line| line.contains("/reset-password#"))
        .expect("a reset link");
    let token = link.split_once('#').expect("a fragment").1.trim();
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_no_content();
    th.anonymous()
        .login(USER1_USERNAME, "New-password-1")
        .await
        .check_no_content();
    assert_eq!(
        settled(&th, &smtp).await.len(),
        1,
        "no confirmation mail for any of them"
    );
    th.teardown().await;
    fake.stop();
    smtp.stop().await;
}

/// The address of a registration is no account's: a reset link is not mailed to it.
#[tokio::test]
async fn a_registration_gets_no_reset_link() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = verifying(&smtp).await;
    th.anonymous()
        .post("/register", &registration("noor", "en"))
        .await
        .check_status(StatusCode::ACCEPTED);
    smtp.received(1).await;
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "noor@example.test"}),
        )
        .await
        .check_no_content();
    let messages = settled(&th, &smtp).await;
    assert_eq!(messages.len(), 1, "the confirmation alone");
    assert!(
        messages[0].decoded.contains("/verify-email#"),
        "{}",
        messages[0].decoded
    );
    th.teardown().await;
    smtp.stop().await;
}

/// The token of the invitation link in `message`.
fn invitation_token(message: &Received) -> String {
    let link = message
        .decoded
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/invite/"))
        .expect("an invitation link in the plain text");
    link.rsplit_once("/invite/")
        .expect("a token")
        .1
        .trim()
        .to_owned()
}

/// A guest link mailed to one address proves that address: who accepts it with a new
/// account at that address is let in at once.
#[tokio::test]
async fn a_new_account_at_the_address_its_guest_link_was_mailed_to_needs_no_confirmation() {
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native().start().await.init_basic().await;
    let th = switched_on(th, &smtp).await;
    let board = th
        .create_board(ROOT_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let links = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(
            &links,
            &json!({"role": "guest", "containerId": board.id, "containerRole": "viewer"}),
        )
        .await
        .created_json();
    th.client
        .post(
            &format!("{links}/mail"),
            &json!({"token": link.token, "recipients": ["olga@example.test"]}),
        )
        .await
        .check_no_content();
    let token = invitation_token(&smtp.received(1).await[0]);
    let accepted = th
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": token, "account": {
                "username": "olga",
                "email": "Olga@example.test",
                "password": "Pa$$word-1",
            }}),
        )
        .await;
    accepted.check_status(StatusCode::OK);
    assert_eq!(
        accepted.value()["containerId"],
        board.id.as_str(),
        "onto its board"
    );
    th.anonymous()
        .login("olga", "Pa$$word-1")
        .await
        .check_no_content();
    assert_eq!(settled(&th, &smtp).await.len(), 1, "the invitation alone");
    th.teardown().await;
    smtp.stop().await;
}
