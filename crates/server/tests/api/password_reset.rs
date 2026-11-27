//! Recovering a password through the local socket and the public completion endpoint.

use std::time::{Duration, Instant};

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{Client, PASSWORD, Setup};
use crate::fake_smtp::FakeSmtp;
use crate::metrics::{sample, scrape_until};
use crate::rooms::{Incoming, PATIENCE, paragraphs, two_in_a_room};

const DELETION: &str = "/users/me/deletion";

#[tokio::test]
async fn reset_links_end_sessions_and_collaborative_rooms_and_cannot_be_replayed() {
    let th = Setup::native().start().await.init_basic().await;
    let mut other_session = th.anonymous();
    other_session
        .login("user1", PASSWORD)
        .await
        .check_no_content();
    let (_, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["before reset"])).await;
    let url = th
        .app
        .password_reset_link("user1")
        .await
        .expect("link issued");
    let token = url.split_once('#').expect("a fragment").1;
    assert_eq!(token.len(), 43, "256 random bits in unpadded base64url");
    let body = json!({"token": token, "password": "New-password-1"});
    th.anonymous()
        .post("/password-reset/complete", &body)
        .await
        .check_no_content();
    assert_eq!(
        one.incoming(PATIENCE).await,
        Incoming::Closed,
        "the editor's session ended"
    );
    th.client
        .get("/users/me")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    other_session
        .get("/users/me")
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.client2
        .get("/users/me")
        .await
        .check_status(StatusCode::OK);
    two.write_acked("the other account remains").await;
    for token in [token, "unknown"] {
        th.anonymous()
            .post(
                "/password-reset/complete",
                &json!({"token":token,"password":"Different-password"}),
            )
            .await
            .check_status(StatusCode::BAD_REQUEST);
    }
    let missing = th
        .anonymous()
        .post(
            "/password-reset/complete",
            &json!({"password": "New-password-1"}),
        )
        .await;
    missing.check_error(
        StatusCode::BAD_REQUEST,
        "the reset link is invalid or expired",
    );
    assert_eq!(missing.value()["errors"][0]["pointer"], json!("/token"));
    th.anonymous()
        .login("user1", PASSWORD)
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    th.anonymous()
        .login("user1", "New-password-1")
        .await
        .check_no_content();
    th.teardown().await;
}

#[cfg(unix)]
#[tokio::test]
async fn only_the_unix_socket_can_issue_reset_links() {
    let socket = std::env::temp_dir().join(format!("fvoci-reset-{}.socket", fvoci_model::new_id()));
    let th = Setup::native()
        .admin_socket(&socket)
        .start()
        .await
        .init_basic()
        .await;
    let path = "/api/v1/admin/users/user1/password-reset";
    let http = reqwest::Client::builder()
        .unix_socket(socket.as_path())
        .build()
        .expect("socket client");
    let answer = http
        .post(format!("http://localhost{path}"))
        .send()
        .await
        .expect("socket responds");
    assert_eq!(answer.status(), StatusCode::OK);
    let body: Value = answer.json().await.expect("link response");
    assert!(
        body["url"]
            .as_str()
            .expect("url")
            .contains("/reset-password#")
    );
    for client in [&th.client, &th.client2, &th.anonymous()] {
        Client::send(client.raw_request(Method::POST, path))
            .await
            .check_status(StatusCode::NOT_FOUND);
    }
    th.teardown().await;
}

/// Who belongs only to an organization that takes no password may still reset their
/// password by mail: every session ends, the new password signs in, and the
/// organization opens for its provider's sessions only, as before.
#[tokio::test]
async fn a_reset_password_does_not_open_an_organization_that_takes_none() {
    use crate::common::{ROOT_WORKSPACE_ID, USER1_USERNAME};
    use crate::fake_oidc::FakeProvider;
    use crate::fake_smtp::FakeSmtp;
    use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
    use crate::sso::{
        ADMIN_SUBJECT, Passwords, admin_through_provider, require_sso, through_provider,
    };

    let fake = FakeProvider::start().await;
    let smtp = FakeSmtp::start_on(0).await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .smtp(smtp.settings())
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
    ts.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "user1@sample.com"}),
        )
        .await
        .check_no_content();
    let mail = smtp.received(1).await;
    let link = mail[0]
        .decoded
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/reset-password#"))
        .expect("the mail has the link");
    let token = link.split_once('#').expect("a fragment").1.trim();
    ts.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_no_content();
    for ended in [&ts.client, &admin] {
        ended
            .get("/users/me")
            .await
            .check_status(StatusCode::UNAUTHORIZED);
    }

    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    let fresh = ts.login(USER1_USERNAME, "New-password-1").await;
    fresh.get("/users/me").await.check_ok();
    let walled = fresh.get(&boards).await;
    walled.check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(walled.error_reason(), "sso_required", "{}", walled.text());
    through_provider(&ts, &fake, &org_id, ADMIN_SUBJECT)
        .await
        .get(&boards)
        .await
        .check_ok();
    ts.teardown().await;
    smtp.stop().await;
    fake.stop();
}

/// A link the operator makes through the socket is recorded once in every organization
/// of the person, as the operator's (on PostgreSQL, user1 is a member of a second
/// organization too, and a third is nobody's here); a refused command, and a reset the
/// person asks for by mail, records nothing.
#[cfg(unix)]
#[tokio::test]
async fn the_operators_reset_link_is_recorded_in_each_organization_of_the_person() {
    use crate::fake_smtp::FakeSmtp;
    use fvoci_model::new_id;
    use fvoci_store::DbType;

    let socket = std::env::temp_dir().join(format!("fvoci-reset-{}.socket", new_id()));
    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .admin_socket(&socket)
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let user1 = th.client.me().await.id;
    let first_id = crate::oidc::org_of(&th).await;
    let first = th.app.store().for_org(&first_id).expect("organization");
    let others = if fvoci_store::testing::db_type_of_the_tests() == DbType::Postgres {
        Some(second_and_third(&th, &user1).await)
    } else {
        None
    };
    let http = reqwest::Client::builder()
        .unix_socket(socket.as_path())
        .build()
        .expect("socket client");
    let link = async |username: &str| {
        http.post(format!(
            "http://localhost/api/v1/admin/users/{username}/password-reset"
        ))
        .send()
        .await
        .expect("socket responds")
        .status()
    };

    let made = link("user1").await;
    let unknown = link("nobody-at-all").await;
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "user1@sample.com"}),
        )
        .await
        .check_no_content();
    smtp.received(1).await;

    assert_eq!((made, unknown), (StatusCode::OK, StatusCode::NOT_FOUND));
    // One record of a link, the operator's, and none of the mailed one.
    let once = |org_id: &str| vec![(true, org_id.to_owned(), user1.clone())];
    assert_eq!(
        reset_links(&first).await,
        once(&first_id),
        "one record, the operator's"
    );
    if let Some(made) = others {
        assert_eq!(
            reset_links(&made[0].1).await,
            once(&made[0].0),
            "also in the second"
        );
        assert_eq!(
            reset_links(&made[1].1).await,
            [],
            "none in an organization not theirs"
        );
    }
    th.teardown().await;
}

/// The records of reset links of the handle's organization, by whomever: whether the
/// server's operator made it, of which organization, to whom.
#[cfg(unix)]
async fn reset_links(store: &fvoci_store::Store) -> Vec<(bool, String, String)> {
    let paging = fvoci_store::Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    store
        .audit_records(fvoci_store::AuditPlace::Organization, paging)
        .await
        .expect("records")
        .into_iter()
        .filter(|record| record.action == fvoci_model::AuditAction::PasswordResetLinkMade)
        .map(|record| {
            let operator = record.actor == fvoci_model::AuditRecordActor::ServerOperator;
            let subject = record.subject.map(|subject| subject.id).unwrap_or_default();
            (operator, record.target_id, subject)
        })
        .collect()
}

/// A second organization, of which `user_id` is made a member, and a third; each with
/// a handle to it.
#[cfg(unix)]
async fn second_and_third(
    th: &crate::common::TestServer,
    user_id: &str,
) -> Vec<(String, fvoci_store::Store)> {
    let mut made = Vec::new();
    for name in ["Second", "Third"] {
        let workspace = fvoci_model::Workspace {
            id: fvoci_model::new_id(),
            ..fvoci_model::Workspace::default()
        };
        let org_id = th
            .app
            .store()
            .system()
            .create_organization(name, &workspace)
            .await
            .expect("another organization");
        let store = th.app.store().for_org(&org_id).expect("organization");
        made.push((org_id, store));
    }
    made[0]
        .1
        .add_organization_member(user_id, fvoci_model::OrgRole::Member)
        .await
        .expect("a member of the second");
    made
}

/// The link of a reset mail, and its token.
fn reset_token(mail: &crate::fake_smtp::Received) -> String {
    let link = mail
        .decoded
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/reset-password#"))
        .expect("the mail has the link");
    link.split_once('#')
        .expect("a fragment")
        .1
        .trim()
        .to_owned()
}

/// Somebody who asked to delete their account and forgot the password asks for a reset
/// by mail, sets a new one, and signs in with it to the way back: the account waits
/// still, until its person takes the deletion back there. The guesses that locked the
/// account before lock it no more.
#[tokio::test]
async fn an_account_that_waits_resets_its_password_and_signs_in_to_the_way_back() {
    use crate::common::USER2_USERNAME;
    use crate::fake_smtp::FakeSmtp;

    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| {
            config.login_account_max_failures = 2;
            config.login_account_lock_seconds = 600;
        })
        .start()
        .await
        .init_basic()
        .await;
    let asked: Value = th
        .client2
        .post(
            DELETION,
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .ok_json();
    for _ in 0..2 {
        th.anonymous()
            .login(USER2_USERNAME, "a forgotten password")
            .await
            .check_unauthorized();
    }
    th.anonymous()
        .login(USER2_USERNAME, PASSWORD)
        .await
        .check_status(StatusCode::TOO_MANY_REQUESTS);
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({"email": "user2@sample.com"}),
        )
        .await
        .check_no_content();
    let mail = smtp.received(1).await;
    assert_eq!(mail[0].recipient, "user2@sample.com");
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": reset_token(&mail[0]), "password": "New-password-1"}),
        )
        .await
        .check_no_content();

    // Unlocked, the new password signs in, and the old one no more.
    let back = th.login(USER2_USERNAME, "New-password-1").await;
    th.anonymous()
        .login(USER2_USERNAME, PASSWORD)
        .await
        .check_unauthorized();
    let me = back.get("/users/me").await;
    me.check_unauthorized();
    assert_eq!(
        me.error_reason(),
        "account_deletion_pending",
        "{}",
        me.text()
    );
    let waiting: Value = back.get(DELETION).await.ok_json();
    assert_eq!(
        waiting["deleteAfter"], asked["deleteAfter"],
        "the reset took nothing back: {waiting}"
    );
    back.delete(DELETION)
        .await
        .check_status(StatusCode::NO_CONTENT);
    back.get("/users/me").await.check_ok();
    th.teardown().await;
    smtp.stop().await;
}

/// The operator's reset link reaches an account that waits to be deleted, and an
/// account whose erasing began gets none: neither a new link, by the operator or by
/// mail, nor anything of one made before it began.
#[tokio::test]
async fn an_account_being_erased_gets_no_reset_and_an_earlier_link_sets_nothing() {
    use crate::common::USER2_USERNAME;
    use crate::fake_smtp::FakeSmtp;

    let smtp = FakeSmtp::start_on(0).await;
    let th = Setup::native()
        .smtp(smtp.settings())
        .start()
        .await
        .init_basic()
        .await;
    let user2 = th.client2.me().await.id;
    th.client2
        .post(
            DELETION,
            &json!({"username": USER2_USERNAME, "password": PASSWORD}),
        )
        .await
        .check_ok();
    let url = th
        .app
        .password_reset_link(USER2_USERNAME)
        .await
        .expect("a link for an account that waits");
    let token = url.split_once('#').expect("a fragment").1.to_owned();

    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET erase_started_at = 1 WHERE id = '{user2}'"
        ))
        .await
        .expect("its erasing began");
    let refused = th.app.password_reset_link(USER2_USERNAME).await;
    assert_eq!(
        refused
            .as_ref()
            .map_err(fvoci_server::error::AppError::status)
            .err(),
        Some(404),
        "no link: {refused:?}"
    );
    let stored = th
        .app
        .store()
        .create_password_reset(
            &user2,
            "user2@sample.com",
            "a-digest",
            fvoci_server::auth::now_millis(),
        )
        .await
        .expect("asked");
    assert!(!stored, "nor one stored");
    // Nor one by mail: asked for first, it is not the message that comes.
    for email in ["user2@sample.com", "user1@sample.com"] {
        th.anonymous()
            .post("/password-reset/request", &json!({ "email": email }))
            .await
            .check_no_content();
    }
    let mail = smtp.received(1).await;
    assert_eq!(mail[0].recipient, "user1@sample.com", "user1's alone");
    th.anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "New-password-1"}),
        )
        .await
        .check_error(
            StatusCode::BAD_REQUEST,
            "the reset link is invalid or expired",
        );
    // The password is as it was: put back as if the erasing had not begun, the old one
    // signs in.
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET erase_started_at = 0 WHERE id = '{user2}'"
        ))
        .await
        .expect("as if it had not begun");
    th.login(USER2_USERNAME, PASSWORD).await;
    th.teardown().await;
    smtp.stop().await;
}

/// A password reset request that mails nothing — for an absent address or an account
/// without a password — still takes the turn of the pace a mailed reset link would:
/// the next message to anybody waits for it, so arrival timing tells nothing about
/// whether an account with a password has that address.
#[tokio::test]
async fn a_reset_that_mails_nothing_takes_its_turn_of_the_pace() {
    const INTERVAL: Duration = Duration::from_secs(1);

    for passwordless_account in [false, true] {
        let smtp = FakeSmtp::start_on(0).await;
        let mut settings = smtp.settings();
        settings.mail_per_second = 1;
        let th = Setup::native()
            .smtp(settings)
            .start()
            .await
            .init_basic()
            .await;
        let probed = if passwordless_account {
            let user2 = th
                .app
                .store()
                .get_user_by_username("user2")
                .await
                .expect("user2");
            th.app
                .store()
                .replace_password_hash(&user2.id, &user2.password_hash, "")
                .await
                .expect("passwordless");
            "user2@sample.com"
        } else {
            "absent@example.test"
        };
        tokio::time::sleep(INTERVAL).await;
        th.anonymous()
            .post("/password-reset/request", &json!({ "email": probed }))
            .await
            .check_no_content();
        assert!(
            th.app.finish_background(Duration::from_secs(10)).await,
            "background pass task finishes"
        );
        let asked = Instant::now();
        th.anonymous()
            .post(
                "/password-reset/request",
                &json!({ "email": "user1@sample.com" }),
            )
            .await
            .check_no_content();
        let mailed = smtp.received(1).await;
        let waited = asked.elapsed();
        assert!(
            waited >= INTERVAL.mul_f32(0.8),
            "passwordless_account={passwordless_account}: the next reset mail came after {waited:?}"
        );
        assert_eq!(mailed.len(), 1);
        assert_eq!(mailed[0].recipient, "user1@sample.com");
        th.teardown().await;
        smtp.stop().await;
    }
}

/// An absent address shares the same per-address hourly quota and drop metric as an
/// existing account, case-insensitively, and stops consuming pace turns once dropped.
#[tokio::test]
async fn an_absent_address_hits_the_hourly_limit_counts_drops_and_stops_taking_pace_turns() {
    const INTERVAL: Duration = Duration::from_secs(1);

    let smtp = FakeSmtp::start_on(0).await;
    let mut settings = smtp.settings();
    settings.mail_per_second = 1;
    settings.mail_reset_per_account_hour = 2;
    let th = Setup::native()
        .smtp(settings)
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    // Two requests to the same absent address in different letter case use its hourly quota.
    for email in ["absent@example.test", "ABSENT@example.test"] {
        th.anonymous()
            .post("/password-reset/request", &json!({ "email": email }))
            .await
            .check_no_content();
    }
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "initial absent requests finish"
    );
    tokio::time::sleep(INTERVAL).await;

    // The third request exceeds the hourly limit: counted as account_limit, no pace turn taken.
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({ "email": "Absent@Example.test" }),
        )
        .await
        .check_no_content();
    let body = scrape_until(&th, |body| {
        sample(
            body,
            "fvoci_password_reset_mail_dropped_total",
            &[("reason", "account_limit")],
        ) == Some(1.0)
    })
    .await;
    assert_eq!(
        sample(
            &body,
            "fvoci_password_reset_mail_dropped_total",
            &[("reason", "account_limit")],
        ),
        Some(1.0),
        "{body}"
    );
    let asked = Instant::now();
    th.anonymous()
        .post(
            "/password-reset/request",
            &json!({ "email": "user1@sample.com" }),
        )
        .await
        .check_no_content();
    let mailed = smtp.received(1).await;
    let waited = asked.elapsed();
    assert!(
        waited < INTERVAL.mul_f32(0.5),
        "dropped request must not consume a pace turn; waited {waited:?}"
    );
    assert_eq!(mailed[0].recipient, "user1@sample.com");
    th.teardown().await;
    smtp.stop().await;
}
