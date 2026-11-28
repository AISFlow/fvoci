//! Who is told, and who reads what they were told: one table of everybody a body may
//! name who does not read it (another organization, another workspace, a guest who
//! was not invited, somebody who left, a reader by link, an account that is gone,
//! one's own space), none of whom is told anything; and one of everything that stops
//! being read after somebody was told of it (the trash, what is gone for good, a
//! comment that went, a board or a workspace that was left), each of which then says
//! nothing.

use std::collections::HashMap;

use fvoci_model::{Block, BlockPatch, BodyChange, Container, ContainerVisibility, Notification};
use fvoci_server::app::tend_notifications;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Making, NotificationReader, Paging, Store};
use serde_json::{Value, json};

use crate::blocks::delete;
use crate::boards::{delete_member, new_board};
use crate::common::{
    HIDDEN_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID,
};
use crate::content_search::set_role;
use crate::notifications::{
    OWNER, board_with_owner, card_with, comment_on, give, give_in, kinds, mentioning, told, unread,
};
use crate::own_space::own_workspace;
use crate::search_pages::{new_page, new_wiki, stored_wiki};
use crate::workspaces::another_organization;

/// How many notifications the user has, read past the API: of somebody who never
/// signs in, too.
async fn held_by(th: &TestServer, user_id: &str) -> usize {
    let all = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(1000).expect("a page size"),
    };
    let reader = NotificationReader {
        user_id,
        making: Making::with_password(),
    };
    th.app
        .store()
        .notifications_of_user(reader, all)
        .await
        .expect("the notifications are read")
        .len()
}

/// How many rows the list of whom to tell has, of every organization.
async fn waiting(th: &TestServer) -> i64 {
    th.app
        .housekeeping()
        .store()
        .waiting_notification_events()
        .await
        .expect("counted")
}

/// Erases the account at once, as the housekeeping does one whose wait ran out.
async fn erase(th: &TestServer, user_id: &str) {
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE users SET deleted_at = 1, delete_after = 1 WHERE id = '{user_id}'"
        ))
        .await
        .expect("the account waits to be deleted");
    th.app.erase_due_accounts().await;
}

/// A wiki open to whoever works in the workspace, stored past the API by `author`,
/// with a page whose body mentions `users`: through whichever handle reaches the
/// workspace. Somebody of the workspace reads the page without being asked in, so
/// that whoever is not told of it is not for want of a role on it.
pub(crate) async fn open_page_mentioning(
    store: &Store,
    workspace_id: &str,
    author: &str,
    users: &[&str],
) {
    let (_, page_id) = stored_wiki(store, workspace_id, ContainerVisibility::Open, author).await;
    let patch = BlockPatch {
        body: BodyChange::Set(
            fvoci_model::Body::parse(mentioning(users))
                .expect("a document")
                .expect("a body"),
        ),
        ..BlockPatch::default()
    };
    store
        .patch_block(&page_id, &patch, author)
        .await
        .expect("the page names them");
}

/// A page with `body` in a new wiki of the test workspace that is open to whoever
/// works there, made by the first client's user. Its identifier.
async fn open_wiki_page(th: &TestServer, body: &Value) -> String {
    let wiki = th
        .client
        .post(&format!("{TEST_WS}/wikis"), &json!({"visibility": "open"}))
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let page = new_page(&th.client, &wiki, "", "An open page", "text").await;
    th.client
        .patch(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{}", page.id),
            &json!({"body": body, "readAt": page.updated_at}),
        )
        .await
        .check_ok();
    page.id
}

/// A user who never signs in and works in the workspace.
async fn stored_member(th: &TestServer, workspace_id: &str, username: &str) -> String {
    let user_id = crate::boards::stored_user(th, username).await;
    th.app
        .store()
        .add_workspace_member(workspace_id, &user_id, fvoci_model::WorkspaceRole::Member)
        .await
        .expect("the user works in the workspace");
    user_id
}

/// Everybody a body may name. Those who read the card are told; nobody else is told
/// anything, whoever they are and whatever they were before.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every way of not reading a card, side by side"
)]
async fn only_who_reads_a_card_is_told_of_being_named_on_it() {
    let th = Setup::native().start().await.init_basic().await;
    let a = th.client.me().await;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    let other_board = board_with_owner(&th, ContainerVisibility::Private).await;

    // Who reads the card: a viewer, and a guest invited to this board.
    let viewer = th.client2.me().await.id;
    give(&th, &board.id, &viewer, "viewer").await;
    let invited_guest = th.add_user("invited").await.me().await.id;
    set_role(&th, &invited_guest, "guest").await;
    give(&th, &board.id, &invited_guest, "commenter").await;

    // Who does not.
    let other_guest = th.add_user("uninvited").await.me().await.id;
    set_role(&th, &other_guest, "guest").await;
    give(&th, &other_board.id, &other_guest, "editor").await;
    let former = th.add_user("former").await.me().await.id;
    give(&th, &board.id, &former, "editor").await;
    delete_member(&th.client, &board.id, &former)
        .await
        .check_no_content();
    // Somebody of the workspace with no role on the board. The board is shared by its
    // link, which lets in whoever has it and makes nobody a reader by name.
    let no_role = th.add_user("norole").await.me().await.id;
    th.share(&board.id).await;
    let elsewhere = th.stranger_in(HIDDEN_WORKSPACE_ID).await;
    // Another organization, where the database holds more than one: its board, and
    // the user of its own who made it.
    let another = if db_type_of_the_tests() == DbType::Postgres {
        let (theirs, their_workspace, their_board) = another_organization(&th).await;
        let owner = theirs
            .get_container(&their_board)
            .await
            .expect("their board")
            .created_by;
        Some((theirs, their_workspace, owner))
    } else {
        None
    };
    let of_another_org = another
        .as_ref()
        .map_or_else(|| UNKNOWN_ID.to_owned(), |(_, _, owner)| owner.clone());
    let ghost = th.add_user("ghost").await.me().await.id;
    give(&th, &board.id, &ghost, "editor").await;
    erase(&th, &ghost).await;

    let read = [("viewer", &viewer), ("invited guest", &invited_guest)];
    let unread_by = [
        ("the writer", &a.id),
        ("a guest invited elsewhere", &other_guest),
        ("who left the board", &former),
        (
            "of the workspace, with no role on the shared board",
            &no_role,
        ),
        ("of another workspace", &elsewhere),
        ("of another organization", &of_another_org),
        ("an account that is gone", &ghost),
    ];
    let mut named: Vec<&str> = read.iter().map(|(_, user)| user.as_str()).collect();
    named.extend(unread_by.iter().map(|(_, user)| user.as_str()));
    named.push(UNKNOWN_ID);

    // Named in the body of a card and in a comment on it, and given the card: the
    // last one at a time, a person property holding one.
    let card = card_with(&th.client, &board.id, &mentioning(&named)).await;
    comment_on(&th.client, &card, &mentioning(&named)).await;
    for user in &named {
        th.client
            .patch(
                &format!("{TEST_WS}/cards/{}", card.id),
                &json!({"updatedProperties": {OWNER: user}}),
            )
            .await
            .check_ok();
    }
    tend_notifications(&th.app).await;
    assert_eq!(waiting(&th).await, 0, "every row was worked off");
    // The two mentions, of one card within the hour, are one notification.
    for (who, user) in read {
        assert_eq!(held_by(&th, user).await, 2, "{who}");
    }
    for (who, user) in unread_by {
        assert_eq!(held_by(&th, user).await, 0, "{who}");
    }
    assert_eq!(held_by(&th, UNKNOWN_ID).await, 0, "nobody");

    // An open board is theirs to join, and told of as theirs to read; an open wiki
    // is read by whoever works in the workspace.
    let open = board_with_owner(&th, ContainerVisibility::Open).await;
    card_with(&th.client, &open.id, &mentioning(&[&no_role, &other_guest])).await;
    let open_names = mentioning(&[&no_role, &other_guest, &elsewhere, &of_another_org, &ghost]);
    open_wiki_page(&th, &open_names).await;
    tend_notifications(&th.app).await;
    assert_eq!(
        held_by(&th, &no_role).await,
        2,
        "of the page of the open wiki, and of the card of the open board"
    );
    assert_eq!(held_by(&th, &other_guest).await, 0, "a guest reads neither");
    for (who, user) in [
        ("of another workspace", &elsewhere),
        ("of another organization", &of_another_org),
        ("an account that is gone", &ghost),
    ] {
        assert_eq!(held_by(&th, user).await, 0, "{who}, named in an open wiki");
    }

    // Written where the one named is not, on a page whoever works there reads: in
    // another workspace of the organization, and in another organization. Who works
    // there is told, so that the page is one that tells.
    let there = stored_member(&th, HIDDEN_WORKSPACE_ID, "there").await;
    let named = [viewer.as_str(), a.id.as_str(), there.as_str()];
    open_page_mentioning(th.app.store(), HIDDEN_WORKSPACE_ID, &elsewhere, &named).await;
    if let Some((theirs, their_workspace, owner)) = &another {
        open_page_mentioning(theirs, their_workspace, owner, &named).await;
    }
    assert!(waiting(&th).await > 0, "they were noted, to be asked about");
    tend_notifications(&th.app).await;
    assert_eq!(waiting(&th).await, 0);
    assert_eq!(
        held_by(&th, &there).await,
        1,
        "who works there, and nowhere else"
    );
    assert_eq!(
        held_by(&th, &viewer).await,
        2,
        "nothing from where they are not"
    );
    assert_eq!(held_by(&th, &a.id).await, 0);

    // One's own space is one's own: whoever is named there is told nothing, and
    // nothing is so much as noted.
    let space = own_workspace(&th.client).await;
    let own: Container = th
        .client
        .create_board(&new_board(&space.id, ContainerVisibility::Private, "Diary"))
        .await
        .created_json();
    let entry: Value = th
        .client
        .post(
            &format!("/w/{}/boards/{}/cards", space.id, own.id),
            &json!({"title": "Today", "body": mentioning(&[&viewer])}),
        )
        .await
        .created_json();
    assert!(entry["id"].is_string(), "{entry}");
    assert_eq!(waiting(&th).await, 0, "nothing is noted in one's own space");
    tend_notifications(&th.app).await;
    assert_eq!(held_by(&th, &viewer).await, 2);

    // The account of somebody who was told goes: so does what they were told.
    erase(&th, &invited_guest).await;
    assert_eq!(
        held_by(&th, &invited_guest).await,
        0,
        "gone with the account"
    );
    th.teardown().await;
}

/// What stops being read after somebody was told of it says nothing from then on: a
/// card in the trash (until it is back), a card gone for good, a page of a wiki in the
/// trash, and everything of a workspace that was left. A comment that went leaves its
/// card, which is still read, without it. What its
/// writer's account going leaves is the notification, by nobody.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every way a card stops being read, side by side"
)]
async fn what_its_reader_reads_no_more_says_nothing_of_itself() {
    let th = Setup::native().start().await.init_basic().await;
    let b = th.client2.me().await;
    let c = th.add_user("third").await;
    let c_id = c.me().await.id;
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    for user in [&b.id, &c_id] {
        give(&th, &board.id, user, "editor").await;
    }
    let body = mentioning(&[&b.id]);
    let kept = card_with(&th.client, &board.id, &body).await;
    let trashed = card_with(&th.client, &board.id, &body).await;
    let purged = card_with(&th.client, &board.id, &body).await;
    let commented = card_with(&th.client, &board.id, &Value::Null).await;
    let comment = comment_on(&th.client, &commented, &body).await;
    let by_ghost = card_with(&c, &board.id, &body).await;
    let wiki = new_wiki(&th.client).await;
    give_in(&th, TEST_WORKSPACE_ID, &wiki, &b.id, "viewer").await;
    let page = new_page(&th.client, &wiki, "", "A page", "text").await;
    th.client
        .patch(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{}", page.id),
            &json!({"body": body, "readAt": page.updated_at}),
        )
        .await
        .check_ok();

    // A page of a wiki open to the workspace, which B reads for working there alone.
    let open_page = open_wiki_page(&th, &body).await;

    let theirs = told(&th, &th.client2).await;
    assert_eq!(kinds(&theirs), (vec!["mention"; 7], 0), "{theirs:?}");
    // Which notification is of which card or page: asked while B reads them all.
    let of: HashMap<String, String> = theirs
        .iter()
        .map(|told| {
            let subject = told.subject.as_ref().expect("read");
            (subject.block_id.clone(), told.id.clone())
        })
        .collect();
    let seen = |told: &[Notification], block_id: &str| {
        let id = &of[block_id];
        told.iter()
            .find(|told| told.id == *id)
            .expect("still listed")
            .subject
            .clone()
    };
    let block = |id: &str| Block {
        id: id.to_owned(),
        container_id: board.id.clone(),
        ..Block::default()
    };

    delete(&th.client, &block(&trashed.id)).await;
    delete(&th.client, &block(&purged.id)).await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM deleted_blocks WHERE id = '{}'",
            purged.id
        ))
        .await
        .expect("the card is gone for good");
    delete(&th.client, &comment).await;
    th.client
        .delete(&format!("{TEST_WS}/containers/{wiki}"))
        .await
        .check_no_content();
    erase(&th, &c_id).await;

    let now = told(&th, &th.client2).await;
    assert_eq!(now.len(), 7, "all are still listed");
    for (what, block_id) in [
        ("in the trash", &trashed.id),
        ("gone for good", &purged.id),
        ("of a wiki in the trash", &page.id),
    ] {
        assert_eq!(seen(&now, block_id), None, "a card or a page {what}");
    }
    // A comment that went takes nothing of its card, which B reads: the card is
    // told of, and the comment no more.
    let of_commented = seen(&now, &commented.id).expect("the card is read");
    assert_eq!(
        (
            of_commented.block_id.as_str(),
            of_commented.comment_id.as_str()
        ),
        (commented.id.as_str(), ""),
        "whose comment went"
    );
    assert!(seen(&now, &kept.id).is_some(), "the one that stays");
    assert!(seen(&now, &open_page).is_some(), "and the open page");
    let orphaned = seen(&now, &by_ghost.id).expect("the card is read as before");
    assert_eq!(
        orphaned.actor_id, c_id,
        "by somebody who is gone, whose identifier says nothing of them"
    );
    assert_eq!(unread(&th.client2).await, 7);

    // Back from the trash: read again.
    crate::blocks::restore_card(&th.client, &board.id, &trashed.id)
        .await
        .check_ok();
    let back = told(&th, &th.client2).await;
    assert!(
        seen(&back, &trashed.id).is_some(),
        "the card that came back"
    );

    // B leaves the workspace: nothing of it says anything any more.
    th.app
        .store()
        .remove_workspace_member(TEST_WORKSPACE_ID, &b.id, "")
        .await
        .expect("B is taken out of the workspace");
    let left = told(&th, &th.client2).await;
    assert_eq!(
        kinds(&left),
        (vec![], 7),
        "the open page too, which nothing but the workspace let B read: {left:?}"
    );
    th.teardown().await;
}

/// An organization that takes only the sessions made through its provider keeps what
/// it told its people from a session of theirs made by a password, as it keeps its
/// boards: such a session lists none of it, counts none, and marks none as read. A
/// session made through the provider finds it as it was.
#[tokio::test]
async fn a_session_its_organization_does_not_take_reaches_nothing_it_was_told_there() {
    use crate::common::ROOT_WORKSPACE_ID;
    use crate::fake_oidc::FakeProvider;
    use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
    use crate::sso::{Passwords, admin_through_provider, require_sso, through_provider, tie};

    let fake = FakeProvider::start().await;
    let th = Setup::local_mode()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    set_provider(&th, &org_id, &fake).await;
    let b = th.client2.me().await.id;
    tie(&th, &th.client2, &fake, "subject-of-user2").await;
    let board: Container = th
        .client
        .create_board(&new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Walled",
        ))
        .await
        .created_json();
    give_in(&th, ROOT_WORKSPACE_ID, &board.id, &b, "viewer").await;
    th.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards/{}/cards", board.id),
            &json!({"title": "Behind the provider", "body": mentioning(&[&b])}),
        )
        .await
        .check_created();
    let open = told(&th, &th.client2).await;
    assert_eq!(kinds(&open), (vec!["mention"], 0), "{open:?}");

    let admin = admin_through_provider(&th, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    assert_eq!(told(&th, &th.client2).await, [], "none is listed");
    assert_eq!(unread(&th.client2).await, 0, "and none is counted");
    th.client2
        .post(&format!("/notifications/{}/read", open[0].id), &Value::Null)
        .await
        .check_error(reqwest::StatusCode::NOT_FOUND, "notification not found");
    th.client2
        .post("/notifications/read", &Value::Null)
        .await
        .check_no_content();
    for path in ["unread", "archive"] {
        th.client2
            .post(
                &format!("/notifications/{}/{path}", open[0].id),
                &Value::Null,
            )
            .await
            .check_error(reqwest::StatusCode::NOT_FOUND, "notification not found");
    }
    th.client2
        .delete(&format!("/notifications/{}/archive", open[0].id))
        .await
        .check_error(reqwest::StatusCode::NOT_FOUND, "notification not found");
    let archived: Vec<Notification> = th
        .client2
        .get("/notifications?archived=true")
        .await
        .ok_json();
    assert_eq!(archived, [], "nothing is in the archive either");

    // Through the provider it is there, and not read: the password session marked
    // nothing.
    let through = through_provider(&th, &fake, &org_id, "subject-of-user2").await;
    let theirs = told(&th, &through).await;
    assert_eq!(kinds(&theirs), (vec!["mention"], 0), "{theirs:?}");
    assert_eq!(theirs[0].read_at, 0, "as it was");
    assert_eq!(unread(&through).await, 1);
    // Read through the provider, it is put away by no other session.
    through
        .post(
            &format!("/notifications/{}/read", theirs[0].id),
            &Value::Null,
        )
        .await
        .check_no_content();
    th.client2
        .post("/notifications/archive", &Value::Null)
        .await
        .check_no_content();
    let still = told(&th, &through).await;
    assert_eq!((still.len(), still[0].archived_at), (1, 0), "{still:?}");
    th.teardown().await;
    fake.stop();
}

/// What is left out is what the organization behind its provider told, and no more:
/// with two organizations, one of which takes only its provider's sessions, a
/// password session lists, counts and marks what the other told; a session made
/// through the other's provider does not open the first either; and a session made
/// through the first's provider reads both.
#[tokio::test]
async fn only_what_the_organization_behind_its_provider_told_is_kept_back() {
    use crate::fake_oidc::FakeProvider;
    use crate::sso::{
        ADMIN_SUBJECT, Passwords, Two, require_sso, through_provider, tie, two_organizations,
    };

    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let Two {
        ts: th,
        ours,
        theirs,
        their_workspace,
    } = two_organizations(&fake).await;
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;

    // Told once in each: by a card of ours, and by an open page of theirs.
    let board = board_with_owner(&th, ContainerVisibility::Private).await;
    give(&th, &board.id, &user2, "editor").await;
    let card = card_with(&th.client2, &board.id, &mentioning(&[&user1])).await;
    let their_store = th.app.store().for_org(&theirs).expect("an identifier");
    let mut author = fvoci_model::User {
        id: fvoci_model::new_id(),
        username: "author.of.theirs".to_owned(),
        email: "author.of.theirs@sample.com".to_owned(),
        ..fvoci_model::User::default()
    };
    their_store
        .create_user(&mut author)
        .await
        .expect("their user is stored");
    open_page_mentioning(&their_store, &their_workspace, &author.id, &[&user1]).await;
    let where_told = |told: &[Notification]| -> Vec<String> {
        let mut places: Vec<String> = told
            .iter()
            .map(|told| {
                let subject = told.subject.as_ref().expect("its reader reads it");
                subject.workspace_id.clone()
            })
            .collect();
        places.sort_unstable();
        places
    };
    let mut both = vec![TEST_WORKSPACE_ID.to_owned(), their_workspace.clone()];
    both.sort_unstable();
    assert_eq!(where_told(&told(&th, &th.client).await), both);

    tie(&th, &th.client, &fake, ADMIN_SUBJECT).await;
    let through_ours = through_provider(&th, &fake, &ours, ADMIN_SUBJECT).await;
    let through_theirs = through_provider(&th, &fake, &theirs, ADMIN_SUBJECT).await;
    require_sso(&through_ours, &ours, &fake, Passwords::Refused)
        .await
        .check_ok();

    // A password: theirs alone, which is marked as read; ours is as it was.
    let with_password = told(&th, &th.client).await;
    let theirs_alone = std::slice::from_ref(&their_workspace);
    assert_eq!(where_told(&with_password), theirs_alone);
    assert_eq!(unread(&th.client).await, 1);
    th.client
        .post("/notifications/read", &Value::Null)
        .await
        .check_no_content();
    assert_eq!(unread(&th.client).await, 0);
    // Their provider: theirs alone too, though it is a provider's session.
    assert_eq!(
        where_told(&told(&th, &through_theirs).await),
        theirs_alone,
        "the session of another organization's provider does not open ours"
    );
    assert_eq!(unread(&through_theirs).await, 0);
    // Our provider: both, ours not read by anything the others did.
    let all = told(&th, &through_ours).await;
    assert_eq!(where_told(&all), both);
    let unread_ones: Vec<&str> = all
        .iter()
        .filter(|told| told.read_at == 0)
        .filter_map(|told| told.subject.as_ref())
        .map(|subject| subject.block_id.as_str())
        .collect();
    assert_eq!(unread_ones, [card.id.as_str()], "ours alone is unread");
    assert_eq!(unread(&through_ours).await, 1);
    th.teardown().await;
    fake.stop();
}
