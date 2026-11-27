//! The sessions made through a provider end with what made them: the identity of their
//! issuer unlinked from the account, or the organization's provider removed. Their
//! connections close with them, and leave the rooms of the cards' bodies they were in.

use fvoci_model::LinkedIdentity;
use serde_json::json;

use crate::common::{Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider};
use crate::oidc::{org_of, server_for, set_provider, with_key_and_private_addresses};
use crate::rooms::{Incoming, Peer, paragraphs, table};
use crate::sso::{ADMIN_SUBJECT, admin_through_provider, through_provider, tie};
use crate::websocket::{Event, PATIENCE, QUIET, Socket, subscribe_to_workspace};

/// What one session holds open: a connection that follows the root workspace, and one
/// in the room of a card's body.
struct Open {
    socket: Socket,
    peer: Peer,
}

impl Open {
    async fn of(ts: &TestServer, client: &Client, card_id: &str, editor: u64) -> Self {
        let user_id = client.me().await.id;
        let mut socket = Socket::connect_as(client).await;
        subscribe_to_workspace(ts, &mut socket, ROOT_WORKSPACE_ID, &user_id).await;
        let mut peer = Peer::connect(client, editor).await;
        peer.joined(TEST_WORKSPACE_ID, card_id).await;
        Self { socket, peer }
    }

    /// Whether the server closed both connections, after what was on its way to them.
    async fn closed(&mut self) -> bool {
        let socket = loop {
            match self.socket.next(PATIENCE).await {
                Event::Closed => break true,
                Event::Nothing => break false,
                Event::Json(_) | Event::Ping => {}
            }
        };
        let peer = loop {
            match self.peer.incoming(PATIENCE).await {
                Incoming::Closed => break true,
                Incoming::Nothing => break false,
                Incoming::Text(_) | Incoming::Frame(_) | Incoming::Caret(_) => {}
            }
        };
        socket && peer
    }

    /// Whether both connections are still open: nothing closes them for a while.
    async fn open(&mut self) -> bool {
        let socket = !matches!(self.socket.next(QUIET).await, Event::Closed);
        let peer = loop {
            match self.peer.incoming(QUIET).await {
                Incoming::Closed => break false,
                Incoming::Nothing => break true,
                Incoming::Text(_) | Incoming::Frame(_) | Incoming::Caret(_) => {}
            }
        };
        socket && peer
    }
}

async fn identities(client: &Client) -> Vec<LinkedIdentity> {
    client.get("/users/me/identities").await.ok_json()
}

async fn unlink(client: &Client, identity: &LinkedIdentity) {
    client
        .delete_json(
            "/users/me/identities",
            &json!({
                "issuer": identity.issuer, "subject": identity.subject, "password": PASSWORD,
            }),
        )
        .await
        .check_no_content();
}

/// Unlinked from a password session, the identity takes the account's sessions made
/// through its issuer along, with their connections and their seats in the rooms; the
/// password session, which said it was not one of them, stays where it was.
#[tokio::test]
async fn unlinking_ends_the_sessions_of_that_issuer_and_what_they_had_open() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let tied = identities(&ts.client).await;
    assert_eq!(tied.len(), 1, "{tied:?}");
    assert!(!tied[0].made_this_session, "the password's session");
    assert!(
        identities(&through).await[0].made_this_session,
        "the provider's"
    );

    let card = table(&ts, &paragraphs(&["first"])).await.card_id;
    let mut of_password = Open::of(&ts, &ts.client, &card, 101).await;
    let mut of_provider = Open::of(&ts, &through, &card, 102).await;
    unlink(&ts.client, &tied[0]).await;

    assert!(of_provider.closed().await, "its connections closed");
    through.get("/users/me").await.check_unauthorized();
    assert!(of_password.open().await, "the password's stay");
    ts.client.get("/users/me").await.check_ok();
    of_password.peer.write_acked("still in the room").await;
    ts.teardown().await;
    fake.stop();
}

/// Unlinked from the session made through its issuer, the identity ends that session
/// too: the request is answered, and the session is over after it.
#[tokio::test]
async fn unlinking_from_the_session_of_that_issuer_ends_it() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let mine = identities(&through).await.remove(0);
    assert!(mine.made_this_session);

    unlink(&through, &mine).await;

    through.get("/users/me").await.check_unauthorized();
    ts.client.get("/users/me").await.check_ok();
    assert_eq!(identities(&ts.client).await, []);
    ts.teardown().await;
    fake.stop();
}

/// An account with two identities of one issuer: unlinking one ends the session made
/// for it, with its connections, and the session made for the other goes on, which
/// alone says it was made for its own.
#[tokio::test]
async fn unlinking_one_identity_of_an_issuer_leaves_the_session_of_the_other() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let first = admin_through_provider(&ts, &fake, &org_id).await;
    tie(&ts, &ts.client, &fake, "another-subject-of-user1").await;
    let second = through_provider(&ts, &fake, &org_id, "another-subject-of-user1").await;
    // Sorted by subject: the two may be tied within one millisecond.
    let made = async |client: &Client| {
        let mut made: Vec<(String, bool)> = identities(client)
            .await
            .into_iter()
            .map(|identity| (identity.subject, identity.made_this_session))
            .collect();
        made.sort();
        made
    };
    assert_eq!(
        made(&first).await,
        [
            ("another-subject-of-user1".to_owned(), false),
            (ADMIN_SUBJECT.to_owned(), true),
        ]
    );
    assert_eq!(
        made(&second).await,
        [
            ("another-subject-of-user1".to_owned(), true),
            (ADMIN_SUBJECT.to_owned(), false),
        ]
    );
    let card = table(&ts, &paragraphs(&["first"])).await.card_id;
    let mut of_first = Open::of(&ts, &first, &card, 101).await;
    let mut of_second = Open::of(&ts, &second, &card, 102).await;
    let of_first_subject = identities(&ts.client)
        .await
        .into_iter()
        .find(|identity| identity.subject == ADMIN_SUBJECT)
        .expect("tied");

    unlink(&ts.client, &of_first_subject).await;

    assert!(
        of_first.closed().await,
        "the first identity's connections closed"
    );
    first.get("/users/me").await.check_unauthorized();
    assert!(of_second.open().await, "the other identity's stay");
    second.get("/users/me").await.check_ok();
    of_second.peer.write_acked("still in the room").await;
    ts.teardown().await;
    fake.stop();
}

/// Removing the organization's provider ends the sessions made through it, of every
/// person, with their connections; a password's go on.
#[tokio::test]
async fn removing_the_provider_ends_its_sessions_and_what_they_had_open() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let card = table(&ts, &paragraphs(&["first"])).await.card_id;
    let mut of_password = Open::of(&ts, &ts.client, &card, 101).await;
    let mut of_provider = Open::of(&ts, &through, &card, 102).await;

    ts.client
        .delete(&format!("/org/{org_id}/oidc"))
        .await
        .check_no_content();

    assert!(of_provider.closed().await, "its connections closed");
    through.get("/users/me").await.check_unauthorized();
    assert!(of_password.open().await, "the password's stay");
    ts.teardown().await;
    fake.stop();
}

/// Saved under another issuer, the provider takes the sessions made through the one
/// before along, with their connections; the password's go on.
#[tokio::test]
async fn another_issuer_ends_the_sessions_of_the_one_before_and_what_they_had_open() {
    let before = FakeProvider::start().await;
    let now = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&before.certificate_der)
        .trusting_oidc_provider(&now.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &before).await;
    let through = admin_through_provider(&ts, &before, &org_id).await;
    let card = table(&ts, &paragraphs(&["first"])).await.card_id;
    let mut of_password = Open::of(&ts, &ts.client, &card, 101).await;
    let mut of_provider = Open::of(&ts, &through, &card, 102).await;

    set_provider(&ts, &org_id, &now).await;

    assert!(of_provider.closed().await, "its connections closed");
    through.get("/users/me").await.check_unauthorized();
    assert!(of_password.open().await, "the password's stay");
    ts.teardown().await;
    before.stop();
    now.stop();
}

/// Another client under the same issuer ends no session: the one made through the
/// client before goes on, and proves nothing of the new one, so it cannot make the
/// organization take only that.
#[tokio::test]
async fn another_client_of_the_same_issuer_ends_nothing_and_is_not_proved() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let through = admin_through_provider(&ts, &fake, &org_id).await;
    let path = format!("/org/{org_id}/oidc");
    let another = |more: serde_json::Value| {
        let mut settings = json!({
            "issuer": fake.issuer, "clientId": "another-client", "clientSecret": CLIENT_SECRET,
        });
        for (name, value) in more.as_object().expect("an object") {
            settings[name] = value.clone();
        }
        settings
    };
    ts.client.put(&path, &another(json!({}))).await.check_ok();
    through.get("/users/me").await.check_ok();
    let unproven = through
        .put(&path, &another(json!({ "ssoRequired": true })))
        .await;
    unproven.check_status(reqwest::StatusCode::CONFLICT);
    assert_eq!(unproven.error_reason(), "sso_unproven");
    // Back to the client it was made through: still another provider, still unproved.
    let back = json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET, "ssoRequired": true });
    let unproven = through.put(&path, &back).await;
    unproven.check_status(reqwest::StatusCode::CONFLICT);
    assert_eq!(unproven.error_reason(), "sso_unproven");
    ts.teardown().await;
    fake.stop();
}
