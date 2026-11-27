//! A sign-in the organization's provider vouches for wins over a registration that
//! waits at the same address: the sign-in is not refused for it, the registration goes
//! with it (its name free, the use of its link given back), and no second account
//! comes of it.

use fvoci_model::Invitation;
use fvoci_store::SignupLookup;
use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ApiResponse, Setup, TestServer};
use crate::email_verification::{SIGNUP_PASSWORD, complete_elsewhere, link_token};
use crate::email_verification_more::{settled, switched_on};
use crate::fake_oidc::{FakeProvider, Person};
use crate::fake_smtp::FakeSmtp;
use crate::oidc::{Browser, Outcome, org_of, set_provider, with_key_and_private_addresses};

/// A server whose one organization has a provider that lets newcomers in, and that
/// confirms the addresses of registrations.
struct Scene {
    th: TestServer,
    smtp: FakeSmtp,
    fake: FakeProvider,
    org_id: String,
}

impl Scene {
    async fn start() -> Self {
        Self::start_on(Setup::native()).await
    }

    async fn start_on(setup: Setup) -> Self {
        let smtp = FakeSmtp::start_on(0).await;
        let fake = FakeProvider::start().await;
        let th = setup
            .trusting_oidc_provider(&fake.certificate_der)
            .configure(with_key_and_private_addresses)
            .start()
            .await
            .init_basic()
            .await;
        let org_id = org_of(&th).await;
        set_provider(&th, &org_id, &fake).await;
        let th = switched_on(th, &smtp).await;
        Self {
            th,
            smtp,
            fake,
            org_id,
        }
    }

    /// A new link of the administrator's, for members.
    async fn link(&self) -> Invitation {
        let workspaces: Vec<fvoci_model::UserWorkspace> =
            self.th.client.get("/users/me/workspaces").await.ok_json();
        self.th
            .client
            .post(
                &format!("/w/{}/invitations", workspaces[0].id),
                &json!({"role": "member"}),
            )
            .await
            .created_json()
    }

    /// Registers `username` at `email` by `link`: it waits for its address.
    async fn register(&self, link: &Invitation, username: &str, email: &str) {
        let accepted = self
            .th
            .anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": link.token, "account": {
                    "username": username, "email": email, "password": SIGNUP_PASSWORD,
                }}),
            )
            .await;
        accepted.check_status(StatusCode::ACCEPTED);
    }

    /// Signs `person` in through the provider.
    async fn sign_in(&self, person: Person) -> Outcome {
        self.fake.sign_in_as(person);
        Browser::new(&self.th, &self.fake)
            .sign_in(&self.org_id)
            .await
    }

    /// Whether a registration with the user name waits.
    async fn waits(&self, username: &str) -> bool {
        self.th
            .app
            .store()
            .live_signup(
                SignupLookup::Username(username),
                fvoci_server::auth::now_millis(),
            )
            .await
            .expect("read")
            .is_some()
    }

    /// How often `link` counts as used.
    async fn uses(&self, link: &Invitation) -> i64 {
        self.th
            .app
            .store()
            .for_org(&self.org_id)
            .expect("the organization")
            .invitation_by_id(&link.id)
            .await
            .expect("read")
            .expect("the link")
            .uses
    }

    /// Opens the newest confirmation link mailed to `email`, with the password.
    async fn confirm(&self, email: &str) -> ApiResponse {
        let messages = settled(&self.th, &self.smtp).await;
        let mail = messages
            .iter()
            .rev()
            .find(|message| {
                message.recipient == email && message.decoded.contains("/verify-email#")
            })
            .expect("its confirmation mail");
        complete_elsewhere(&self.th, &link_token(mail), SIGNUP_PASSWORD).await
    }

    async fn accounts(&self) -> i64 {
        self.th
            .app
            .store()
            .get_registered_user_count()
            .await
            .expect("counted")
    }

    async fn stop(self) {
        self.th.teardown().await;
        self.fake.stop();
        self.smtp.stop().await;
    }
}

/// Somebody registered with the person's address (they, or somebody else). The person
/// signs in through the provider for the first time: let in, under the very name the
/// registration held, and the registration is gone with what it took.
#[tokio::test]
async fn a_first_sign_in_through_the_provider_removes_the_registration_at_its_address() {
    let scene = Scene::start().await;
    let link = scene.link().await;
    scene.register(&link, "mo", "mo@example.com").await;
    assert_eq!(scene.uses(&link).await, 1, "counted from the start");
    let before = scene.accounts().await;

    // In another letter case, as addresses are compared.
    let mut person = Person::new("subject-mo", "Mo@Example.com");
    person.preferred_username = Some("mo".to_owned());
    let signed_in = scene.sign_in(person).await;
    let me = scene.th.client_with_token(signed_in.token()).me().await;
    assert_eq!(
        me.username, "mo",
        "the name is free for the person it is of"
    );
    assert!(!scene.waits("mo").await, "the registration went");
    assert_eq!(
        scene.uses(&link).await,
        0,
        "its use of the link is given back"
    );
    assert_eq!(scene.accounts().await, before + 1, "one account");

    // What the registration left confirms nothing, and nothing signs in by its password.
    let ended = scene.confirm("mo@example.com").await;
    ended.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(ended.refused_at(), "/token", "an ended link");
    scene
        .th
        .anonymous()
        .login("mo", SIGNUP_PASSWORD)
        .await
        .check_status(StatusCode::UNAUTHORIZED);
    assert_eq!(scene.accounts().await, before + 1, "still one");
    scene.stop().await;
}

/// The provider's name for a newcomer is held by a registration at another address:
/// the newcomer is let in under another name, and that registration waits on.
#[tokio::test]
async fn a_name_a_registration_holds_is_not_taken_from_it_by_a_newcomer() {
    let scene = Scene::start().await;
    let link = scene.link().await;
    scene.register(&link, "sam", "sam@elsewhere.test").await;

    let mut person = Person::new("subject-sam", "samantha@example.com");
    person.preferred_username = Some("sam".to_owned());
    let signed_in = scene.sign_in(person).await;
    let me = scene.th.client_with_token(signed_in.token()).me().await;
    assert!(
        me.username.starts_with("sam-") && me.username != "sam",
        "another name: {}",
        me.username
    );
    assert!(
        scene.waits("sam").await,
        "the other person's registration stays"
    );
    assert_eq!(scene.uses(&link).await, 1, "with its use of the link");
    scene
        .confirm("sam@elsewhere.test")
        .await
        .check_status(StatusCode::OK);
    scene.stop().await;
}

/// The person has an account through the provider already, whose address holds no
/// place: somebody registers with it. At the person's next sign-in the registration
/// goes; a provider that does not vouch for the address removes nothing.
#[tokio::test]
async fn a_later_sign_in_removes_a_registration_made_at_the_address_since() {
    let scene = Scene::start().await;
    let person = Person::new("subject-ann", "ann@example.com");
    let first = scene.sign_in(person.clone()).await;
    let ann = scene.th.client_with_token(first.token()).me().await;
    let before = scene.accounts().await;

    let link = scene.link().await;
    scene.register(&link, "ann-too", "ANN@example.com").await;
    assert_eq!(scene.uses(&link).await, 1, "it waits");

    // Not vouched for: the address is said to be unverified, or not said at all.
    let unverified = Person {
        email_verified: Some(false),
        ..person.clone()
    };
    let unsaid = Person {
        email: None,
        ..person.clone()
    };
    for unproved in [unverified, unsaid] {
        let again = scene.sign_in(unproved).await;
        assert_eq!(
            scene.th.client_with_token(again.token()).me().await.id,
            ann.id,
            "the same account"
        );
        assert!(scene.waits("ann-too").await, "nothing is removed");
    }

    let again = scene.sign_in(person).await;
    assert_eq!(
        scene.th.client_with_token(again.token()).me().await.id,
        ann.id,
        "the same account"
    );
    assert!(!scene.waits("ann-too").await, "the registration went");
    assert_eq!(scene.uses(&link).await, 0, "and gave its use back");
    assert_eq!(scene.accounts().await, before, "no second account");
    scene.stop().await;
}

/// A confirmation and a first sign-in through the provider at once, for one address:
/// one of them makes the account, never both.
#[tokio::test]
async fn a_confirmation_and_a_first_sign_in_at_once_make_one_account() {
    let scene = Scene::start().await;
    let link = scene.link().await;
    for round in 0..4 {
        let (name, email) = (format!("race{round}"), format!("race{round}@example.com"));
        scene.register(&link, &name, &email).await;
        let before = scene.accounts().await;
        let messages = settled(&scene.th, &scene.smtp).await;
        let mail = messages
            .iter()
            .rev()
            .find(|message| message.recipient == email)
            .expect("its confirmation mail");
        let token = link_token(mail);
        scene
            .fake
            .sign_in_as(Person::new(&format!("subject-race{round}"), &email));
        let browser = Browser::new(&scene.th, &scene.fake);
        let (confirmed, signed_in) = tokio::join!(
            complete_elsewhere(&scene.th, &token, SIGNUP_PASSWORD),
            browser.sign_in(&scene.org_id),
        );
        let by_provider = matches!(signed_in, Outcome::SignedIn(_));
        let by_mail = confirmed.status == StatusCode::OK;
        assert!(
            by_provider != by_mail,
            "round {round}: one of the two, not {signed_in:?} and {}",
            confirmed.text()
        );
        if by_mail {
            assert_eq!(
                signed_in,
                Outcome::Refused("oidc-email-taken".to_owned()),
                "the address is an account's by then"
            );
        } else {
            confirmed.check_status(StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            scene.accounts().await,
            before + 1,
            "round {round}: one account"
        );
        assert!(!scene.waits(&name).await, "round {round}: nothing waits");
    }
    scene.stop().await;
}

/// A provider's word is worth nothing outside its organization: a registration by a
/// link of another organization, and one of its own, wait on when somebody signs in
/// through the provider at their address. On a server of several organizations.
#[tokio::test]
async fn a_provider_removes_no_registration_outside_its_organization() {
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let scene = Scene::start_on(Setup::native().configure(|config| {
        fvoci_server::config::ORG_CREATION_OPEN.clone_into(&mut config.org_creation);
    }))
    .await;
    // Another organization, and a link of its founder's.
    let th = &scene.th;
    th.anonymous()
        .post(
            "/register",
            &crate::email_verification::registration("bea", "en"),
        )
        .await
        .check_status(StatusCode::ACCEPTED);
    let founded = scene.confirm("bea@example.test").await;
    founded.check_status(StatusCode::OK);
    let bea = th.client_with_token(&founded.session_cookie().expect("a session"));
    let workspace_id = founded.value()["workspaceId"]
        .as_str()
        .expect("hers")
        .to_owned();
    let theirs: Invitation = bea
        .post(
            &format!("/w/{workspace_id}/invitations"),
            &json!({"role": "member"}),
        )
        .await
        .created_json();
    scene.register(&theirs, "cross", "cross@example.com").await;
    let mut own = crate::email_verification::registration("own", "en");
    own["email"] = json!("own@example.com");
    th.anonymous()
        .post("/register", &own)
        .await
        .check_status(StatusCode::ACCEPTED);

    for (subject, email) in [
        ("subject-cross", "cross@example.com"),
        ("subject-own", "own@example.com"),
    ] {
        let signed_in = scene.sign_in(Person::new(subject, email)).await;
        assert!(
            matches!(signed_in, Outcome::SignedIn(_)),
            "not refused: {signed_in:?}"
        );
    }
    assert!(scene.waits("cross").await, "another organization's stays");
    assert!(scene.waits("own").await, "one of its own stays");
    scene
        .confirm("cross@example.com")
        .await
        .check_status(StatusCode::OK);
    scene.stop().await;
}
