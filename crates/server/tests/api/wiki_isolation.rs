//! Who a wiki and its pages are there for: under their own workspace and in their own
//! organization only, for a guest only where they were invited, not for the
//! administrator of the workspace who only manages the wiki, and for each role on the
//! wiki what that role is let do. Who a wiki is not there for is answered as for no
//! wiki, over HTTP and at the door of a page's room.

use fvoci_model::{
    Container, ContainerKind, ContainerMember, ContainerVisibility, NewPage, WorkspaceRole,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{
    Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS,
    UNKNOWN_ID,
};
use crate::presence::visit;
use crate::rooms::Peer;
use crate::websocket::{QUIET, Socket};

/// What the five operations of pages answer, in the order of [`asked`].
type Answers = [u16; 5];

/// Every operation of pages is answered as for no wiki.
const NOT_THERE: Answers = [404; 5];
/// The tree and a page are read, and nothing is changed: a reader of the wiki.
const READ_ONLY: Answers = [200, 200, 403, 403, 403];
/// Everything is done: an editor of the wiki.
const ALL_DONE: Answers = [200, 200, 201, 200, 204];

/// What every wiki of [`wiki_of`] says of itself: for its readers, not for who only
/// manages it.
const DESCRIPTION: &str = "What this wiki is for";

/// A wiki of the test workspace made by `maker`, of the visibility `visibility` (`private` for the
/// invited only, `open` for everybody in the workspace), with a page: the identifiers of
/// both.
pub(crate) async fn wiki_of(maker: &Client, visibility: &str) -> (String, String) {
    let wiki = json!({"visibility": visibility, "description": DESCRIPTION});
    let wiki = maker.post(&format!("{TEST_WS}/wikis"), &wiki).await;
    wiki.check_created();
    let wiki = wiki.value()["id"].as_str().expect("an id").to_owned();
    let page = maker
        .post(
            &format!("{TEST_WS}/wikis/{wiki}/pages"),
            &json!({"title": "A page"}),
        )
        .await;
    page.check_created();
    let page = page.value()["id"].as_str().expect("an id").to_owned();
    (wiki, page)
}

/// Asks the five operations of pages as `client`, under the workspace path `at`: the
/// tree, the page, a new page, a change of the page, and last the deletion of the
/// page. The statuses, in that order.
async fn asked(client: &Client, at: &str, wiki_id: &str, page_id: &str) -> Answers {
    let pages = format!("{at}/wikis/{wiki_id}/pages");
    let page = format!("{pages}/{page_id}");
    [
        client.get(&pages).await,
        client.get(&page).await,
        client.post(&pages, &json!({"title": "made"})).await,
        client.patch(&page, &json!({"title": "changed"})).await,
        client.delete(&page).await,
    ]
    .map(|response| response.status.as_u16())
}

/// The wikis `client` lists under the workspace path `at`, by identifier.
async fn listed(client: &Client, at: &str) -> Vec<String> {
    let wikis: Vec<Container> = client.get(&format!("{at}/wikis")).await.ok_json();
    assert!(
        wikis.iter().all(|wiki| wiki.kind == ContainerKind::Wiki),
        "the list of wikis has a board: {wikis:?}"
    );
    wikis.into_iter().map(|wiki| wiki.id).collect()
}

/// Whether a connection of `client` is let into the room of the page under the
/// workspace `workspace_id`, and how: `write`, `read`, or the reason it is refused
/// with.
async fn at_the_door(client: &Client, workspace_id: &str, page_id: &str) -> String {
    let mut peer = Peer::connect(client, 301).await;
    let answer = peer.join(workspace_id, page_id).await;
    let said = if answer["type"] == json!("docJoined") {
        &answer["mode"]
    } else {
        &answer["reason"]
    };
    said.as_str().expect("a mode or a reason").to_owned()
}

/// Puts `user_id` on the wiki with `role` (`viewer`, `commenter` or `editor`), as
/// `admin`, who administers it.
pub(crate) async fn put_on(admin: &Client, wiki_id: &str, user_id: &str, role: &str) {
    assert!(
        ["viewer", "commenter", "editor"].contains(&role),
        "no such role: {role}"
    );
    let member = ContainerMember {
        container_id: wiki_id.to_owned(),
        user_id: user_id.to_owned(),
        role: role.parse().expect("a valid role"),
        synthetic: false,
        inherited_role: None,
    };
    let path = format!("{TEST_WS}/containers/{wiki_id}/members");
    admin.post(&path, &member).await.check_ok();
    // Adding who is on it already changes no role: the role is set.
    admin
        .put(&format!("{path}/{user_id}"), &member)
        .await
        .check_ok();
}

/// A wiki is reached under the workspace it is in, by who is on it too: under another
/// workspace they work in, and under one they do not, it is answered as no wiki is.
#[tokio::test]
async fn a_wiki_and_its_pages_are_under_their_own_workspace_only() {
    let th = Setup::native().start().await.init_basic().await;
    let (wiki, page) = wiki_of(&th.client, "open").await;
    assert!(listed(&th.client, TEST_WS).await.contains(&wiki));

    // Under the root workspace, which the user works in, it is the wiki that is not
    // there; under the hidden one the workspace is not, before any wiki is asked for.
    let hidden = format!("/w/{HIDDEN_WORKSPACE_ID}");
    for at in [ROOT_WS, hidden.as_str()] {
        assert_eq!(asked(&th.client, at, &wiki, &page).await, NOT_THERE, "{at}");
        // As for a wiki that is nowhere, to the letter.
        for path in ["/pages", &format!("/pages/{page}")] {
            let of_wiki = th.client.get(&format!("{at}/wikis/{wiki}{path}")).await;
            let of_none = th
                .client
                .get(&format!("{at}/wikis/{UNKNOWN_ID}{path}"))
                .await;
            assert_eq!(of_wiki.problem(), of_none.problem(), "{at}{path}");
        }
        // Nor is it there as a board, for what a wiki and a board share.
        for shared in ["", "/members"] {
            th.client
                .get(&format!("{at}/boards/{wiki}{shared}"))
                .await
                .check_not_found();
        }
    }
    assert!(!listed(&th.client, ROOT_WS).await.contains(&wiki));
    for workspace in [ROOT_WORKSPACE_ID, HIDDEN_WORKSPACE_ID, UNKNOWN_ID] {
        assert_eq!(
            at_the_door(&th.client, workspace, &page).await,
            "not_found",
            "the room, named under {workspace}"
        );
    }

    // Under its own workspace all of it is there, and nothing above was changed.
    assert_eq!(
        at_the_door(&th.client, TEST_WORKSPACE_ID, &page).await,
        "write"
    );
    assert_eq!(asked(&th.client, TEST_WS, &wiki, &page).await, ALL_DONE);
    th.teardown().await;
}

/// A wiki of another organization is not there, whatever workspace is named; who is
/// let into its workspace and onto the wiki reaches it there, and nowhere else, and
/// reaches nothing of their own organization through it. What is asked here is
/// answered by the workspace a request names; that the rows of two organizations stay
/// apart whatever is named is the store's test
/// (`the_pages_of_two_organizations_are_out_of_each_others_reach`).
#[tokio::test]
async fn a_wiki_of_another_organization_is_reached_through_its_workspace_only() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let (theirs, their_workspace, their_board) = crate::workspaces::another_organization(&th).await;
    let members = theirs.get_members_for_container(&their_board).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut wiki = Container {
        id: fvoci_model::new_id(),
        workspace_id: their_workspace.clone(),
        visibility: ContainerVisibility::Open,
        kind: ContainerKind::Wiki,
        title: "theirs".to_owned(),
        ..Container::default()
    };
    theirs
        .insert_container_with_admin(&mut wiki, &owner)
        .await
        .expect("their wiki is stored");
    let page = theirs
        .create_page(
            &wiki.id,
            &NewPage::default(),
            fvoci_model::PageLimits::default(),
            &owner,
        )
        .await
        .expect("their page is stored")
        .page;
    let (wiki, page) = (wiki.id, page.id);
    let (our_wiki, our_page) = wiki_of(&th.client, "open").await;

    let there = format!("/w/{their_workspace}");
    for at in [TEST_WS, there.as_str()] {
        assert_eq!(asked(&th.client, at, &wiki, &page).await, NOT_THERE, "{at}");
        let of_theirs = th.client.get(&format!("{at}/wikis/{wiki}/pages")).await;
        let of_none = th
            .client
            .get(&format!("{at}/wikis/{UNKNOWN_ID}/pages"))
            .await;
        assert_eq!(of_theirs.problem(), of_none.problem(), "{at}");
        th.client
            .get(&format!("{at}/containers/{wiki}"))
            .await
            .check_not_found();
    }
    for workspace in [TEST_WORKSPACE_ID, their_workspace.as_str()] {
        assert_eq!(at_the_door(&th.client, workspace, &page).await, "not_found");
    }
    assert!(!listed(&th.client, TEST_WS).await.contains(&wiki));

    // Let into their workspace as a guest and onto the wiki, the user reaches it under
    // that workspace, in that organization, and still not under their own.
    let user1 = th.client.me().await.id;
    theirs
        .add_workspace_member(&their_workspace, &user1, WorkspaceRole::Guest)
        .await
        .expect("let into their workspace");
    let member = ContainerMember {
        container_id: wiki.clone(),
        user_id: user1,
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    theirs.save_member(&member).await.expect("invited");
    assert_eq!(
        listed(&th.client, &there).await,
        std::slice::from_ref(&wiki)
    );
    assert_eq!(asked(&th.client, TEST_WS, &wiki, &page).await, NOT_THERE);
    assert_eq!(
        at_the_door(&th.client, TEST_WORKSPACE_ID, &page).await,
        "not_found"
    );
    // Nor is the user's own wiki, of their own organization, there under theirs.
    assert_eq!(
        asked(&th.client, &there, &our_wiki, &our_page).await,
        NOT_THERE
    );
    assert_eq!(
        at_the_door(&th.client, &their_workspace, &our_page).await,
        "not_found"
    );
    assert_eq!(
        at_the_door(&th.client, &their_workspace, &page).await,
        "write"
    );
    assert_eq!(asked(&th.client, &there, &wiki, &page).await, ALL_DONE);
    let made = theirs.get_page_tree(&wiki).await.expect("their tree");
    assert_eq!(
        made.len(),
        1,
        "the page made is theirs, the one deleted gone"
    );
    th.teardown().await;
}

/// A guest of the workspace has the wikis they were invited to, one by one: a wiki for
/// everybody in the workspace is not for them, and is answered as no wiki is.
#[tokio::test]
async fn a_guest_has_only_the_wikis_it_was_invited_to() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = crate::guests::guest_of_the_workspace(&th).await;
    let (invited, invited_page) = wiki_of(&th.client, "private").await;
    let (open, open_page) = wiki_of(&th.client, "open").await;
    let (private, private_page) = wiki_of(&th.client, "private").await;
    put_on(&th.client, &invited, &guest, "viewer").await;

    assert_eq!(
        listed(&th.client2, TEST_WS).await,
        std::slice::from_ref(&invited)
    );
    let of_none = th
        .client2
        .get(&format!("{TEST_WS}/wikis/{UNKNOWN_ID}/pages"))
        .await;
    for (wiki, page) in [(&open, &open_page), (&private, &private_page)] {
        assert_eq!(asked(&th.client2, TEST_WS, wiki, page).await, NOT_THERE);
        let of_wiki = th
            .client2
            .get(&format!("{TEST_WS}/wikis/{wiki}/pages"))
            .await;
        assert_eq!(of_wiki.problem(), of_none.problem(), "wiki {wiki}");
        assert_eq!(
            at_the_door(&th.client2, TEST_WORKSPACE_ID, page).await,
            "not_found"
        );
        // Nor does the guest join it, open as it is to the members of the workspace.
        th.client2
            .post(&format!("{TEST_WS}/containers/{wiki}/join"), &json!({}))
            .await
            .check_not_found();
    }

    // Where it was invited, it does what its role lets it.
    assert_eq!(
        at_the_door(&th.client2, TEST_WORKSPACE_ID, &invited_page).await,
        "read"
    );
    assert_eq!(
        asked(&th.client2, TEST_WS, &invited, &invited_page).await,
        READ_ONLY
    );
    put_on(&th.client, &invited, &guest, "editor").await;
    assert_eq!(
        at_the_door(&th.client2, TEST_WORKSPACE_ID, &invited_page).await,
        "write"
    );
    assert_eq!(
        asked(&th.client2, TEST_WS, &invited, &invited_page).await,
        ALL_DONE
    );
    th.teardown().await;
}

/// The administrator of a workspace manages a wiki for the invited only that they are
/// not on: they list it, read what it is called and who is on it, and may put
/// themselves on it. Its pages are not there for them until they do.
#[tokio::test]
async fn who_only_manages_a_wiki_reads_none_of_its_pages() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = crate::workspace_admin::workspace_admin(&th).await;
    let (maker, admin_client) = (&th.client, &th.client2);
    let (wiki, page) = wiki_of(maker, "private").await;

    assert!(listed(admin_client, TEST_WS).await.contains(&wiki));
    let seen: Container = admin_client
        .get(&format!("{TEST_WS}/containers/{wiki}"))
        .await
        .ok_json();
    assert_eq!(
        (seen.kind, seen.id.as_str(), seen.description.as_str()),
        (ContainerKind::Wiki, wiki.as_str(), ""),
        "what the wiki says of itself is for its readers"
    );
    let of_maker: Container = maker
        .get(&format!("{TEST_WS}/containers/{wiki}"))
        .await
        .ok_json();
    assert_eq!(of_maker.description, DESCRIPTION);
    // The link that shows the wiki's pages is of its content.
    admin_client
        .get(&format!("{TEST_WS}/containers/{wiki}/public-link"))
        .await
        .check_not_found();
    admin_client
        .get(&format!("{TEST_WS}/containers/{wiki}/members"))
        .await
        .check_ok();
    assert_eq!(asked(admin_client, TEST_WS, &wiki, &page).await, NOT_THERE);
    let of_wiki = admin_client
        .get(&format!("{TEST_WS}/wikis/{wiki}/pages"))
        .await;
    let of_none = admin_client
        .get(&format!("{TEST_WS}/wikis/{UNKNOWN_ID}/pages"))
        .await;
    assert_eq!(of_wiki.problem(), of_none.problem());
    assert_eq!(
        at_the_door(admin_client, TEST_WORKSPACE_ID, &page).await,
        "not_found"
    );
    // The maker's page is as it was, and no other was made.
    let tree: Value = maker
        .get(&format!("{TEST_WS}/wikis/{wiki}/pages"))
        .await
        .value();
    assert_eq!(tree.as_array().map(Vec::len), Some(1), "{tree}");
    assert_eq!(tree[0]["title"], json!("A page"));

    // On the wiki, by their own hand, they read and write it.
    put_on(admin_client, &wiki, &admin, "editor").await;
    assert_eq!(
        at_the_door(admin_client, TEST_WORKSPACE_ID, &page).await,
        "write"
    );
    assert_eq!(asked(admin_client, TEST_WS, &wiki, &page).await, ALL_DONE);
    th.teardown().await;
}

/// On a wiki a member does what their role lets them, as on a board: off a wiki for
/// the invited only there is no wiki, a wiki for everybody in the workspace is read by
/// who did not join it and written in by who did, a viewer and a commenter read, and an
/// editor writes.
#[tokio::test]
async fn the_pages_of_a_wiki_are_for_each_role_what_the_role_is_let_do() {
    let th = Setup::native().start().await.init_basic().await;
    let member = th.client2.me().await.id;
    let (private, private_page) = wiki_of(&th.client, "private").await;
    let (open, open_page) = wiki_of(&th.client, "open").await;

    assert_eq!(
        asked(&th.client2, TEST_WS, &private, &private_page).await,
        NOT_THERE,
        "off a wiki for the invited only"
    );
    assert_eq!(
        at_the_door(&th.client2, TEST_WORKSPACE_ID, &private_page).await,
        "not_found"
    );
    assert_eq!(
        listed(&th.client2, TEST_WS).await,
        std::slice::from_ref(&open)
    );
    // A wiki for everybody in the workspace is read by who did not join it, and written
    // in by who did: until then its pages are read, and every change is refused.
    assert_eq!(
        asked(&th.client2, TEST_WS, &open, &open_page).await,
        READ_ONLY,
        "a member of the workspace who did not join the open wiki"
    );
    let door = at_the_door(&th.client2, TEST_WORKSPACE_ID, &open_page).await;
    assert_eq!(door, "read");
    th.client2
        .post(&format!("{TEST_WS}/containers/{open}/join"), &json!({}))
        .await
        .check_ok();
    let door = at_the_door(&th.client2, TEST_WORKSPACE_ID, &open_page).await;
    let joined = asked(&th.client2, TEST_WS, &open, &open_page).await;
    assert_eq!(
        (door.as_str(), joined),
        ("write", ALL_DONE),
        "who joined it"
    );

    for role in ["viewer", "commenter"] {
        put_on(&th.client, &private, &member, role).await;
        assert_eq!(
            asked(&th.client2, TEST_WS, &private, &private_page).await,
            READ_ONLY,
            "{role}"
        );
        assert_eq!(
            at_the_door(&th.client2, TEST_WORKSPACE_ID, &private_page).await,
            "read",
            "{role}"
        );
    }
    // A change is refused to a reader the same whether there is such a page or not.
    let pages = format!("{TEST_WS}/wikis/{private}/pages");
    let change = json!({"title": "changed"});
    let of_page = th
        .client2
        .patch(&format!("{pages}/{private_page}"), &change)
        .await;
    let of_none = th
        .client2
        .patch(&format!("{pages}/{UNKNOWN_ID}"), &change)
        .await;
    of_page.check_forbidden();
    assert_eq!(of_page.problem(), of_none.problem());
    // A page of a wiki the reader is not on is no page of the wiki they read: asked
    // for by that wiki's path, it is answered as a page that is nowhere.
    let (_, hidden_page) = wiki_of(&th.client, "private").await;
    let of_none = th.client2.get(&format!("{pages}/{UNKNOWN_ID}")).await;
    of_none.check_not_found();
    for no_page in [hidden_page.as_str(), private.as_str()] {
        let read = th.client2.get(&format!("{pages}/{no_page}")).await;
        assert_eq!(read.problem(), of_none.problem(), "page {no_page}");
    }

    put_on(&th.client, &private, &member, "editor").await;
    assert_eq!(
        at_the_door(&th.client2, TEST_WORKSPACE_ID, &private_page).await,
        "write"
    );
    assert_eq!(
        asked(&th.client2, TEST_WS, &private, &private_page).await,
        ALL_DONE
    );
    th.teardown().await;
}

/// Who may not read a wiki is told nothing of its pages: not what is made, changed
/// and deleted in it, not who visits it, and not that it went to the trash. A member
/// of the workspace who is not on it, a guest who was not invited to it, and the
/// administrator who only manages it, each beside a wiki they do read, of which they
/// are told.
#[tokio::test]
async fn who_may_not_read_a_wiki_is_told_nothing_of_its_pages() {
    for standing in ["member", "guest", "administrator"] {
        let th = Setup::native().start().await.init_basic().await;
        let other = match standing {
            "member" => th.client2.me().await.id,
            "guest" => crate::guests::guest_of_the_workspace(&th).await,
            "administrator" => crate::workspace_admin::workspace_admin(&th).await,
            unknown => unreachable!("no such standing: {unknown}"),
        };
        let (hidden, hidden_page) = wiki_of(&th.client, "private").await;
        let (shown, shown_page) = wiki_of(&th.client, "private").await;
        put_on(&th.client, &shown, &other, "viewer").await;
        let mut listener =
            WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &other).await;

        // A page is changed, made and deleted in the wiki they do not read, and then
        // one changed in the wiki they do: what comes before that is all there is.
        let pages = format!("{TEST_WS}/wikis/{hidden}/pages");
        let change = json!({"title": "changed"});
        th.client
            .patch(&format!("{pages}/{hidden_page}"), &change)
            .await
            .check_ok();
        let made = th.client.post(&pages, &json!({"title": "made"})).await;
        made.check_created();
        let made = made.value()["id"].as_str().expect("an id").to_owned();
        th.client
            .delete(&format!("{pages}/{made}"))
            .await
            .check_no_content();
        th.client
            .patch(
                &format!("{TEST_WS}/wikis/{shown}/pages/{shown_page}"),
                &change,
            )
            .await
            .check_ok();
        let mut heard = Vec::new();
        for _ in 0..20 {
            heard.extend(listener.drain().await);
            if heard
                .iter()
                .any(|told| told["block"]["id"] == json!(shown_page))
            {
                break;
            }
        }
        let of_pages: Vec<&Value> = heard
            .iter()
            .filter(|told| told["type"] == json!("blockChanged"))
            .collect();
        assert_eq!(of_pages.len(), 1, "{standing}: {of_pages:?}");
        assert_eq!(
            (
                &of_pages[0]["block"]["id"],
                &of_pages[0]["block"]["containerId"]
            ),
            (&json!(shown_page), &json!(shown)),
            "{standing}: told of the page they read, and of no other"
        );

        // A visit to it is refused, and says nothing of who else is there.
        let mut owner = Socket::connect_as(&th.client).await;
        let mut visitor = Socket::connect_as(&th.client2).await;
        visit(&mut owner, &hidden, &hidden_page).await;
        visit(&mut visitor, &hidden, &hidden_page).await;
        assert!(
            visitor.next_json(QUIET).await.is_none(),
            "{standing}: no roster of a wiki they do not read"
        );

        // In the trash it is for who administered it, and for who manages the
        // workspace by its name: not for a member or a guest it was hidden from.
        th.client
            .delete(&format!("{TEST_WS}/containers/{hidden}"))
            .await
            .check_no_content();
        let trash = th.client2.get(&format!("{TEST_WS}/trash/containers")).await;
        let listed = trash.status == StatusCode::OK && trash.text().contains(&hidden);
        assert_eq!(
            listed,
            standing == "administrator",
            "{standing}: {}",
            trash.text()
        );
        assert!(
            !trash.text().contains(DESCRIPTION),
            "{standing}: what the wiki says of itself is for its readers"
        );
        th.teardown().await;
    }
}
