//! What the references of a body name (`resolveReferences`): told to who reads the card
//! or the page, as things are now, and to nobody else. What a user does not read is
//! answered as what is not there, to the letter, whoever they are: of another
//! workspace or organization, a guest who was not invited, no member of what is for the
//! invited only, an administrator who only manages it, or without a session.

use fvoci_model::{
    Block, BlockType, Container, ContainerKind, ContainerMember, ContainerVisibility, NewPage,
    PageLimits,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{insert, restore_card, trash_card};
use crate::common::{
    ApiResponse, Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID,
};

/// What the title of everything a test hides has in it: no answer to who does not read
/// it may hold this.
pub(crate) const HIDDEN: &str = "hidden-title";

/// A board and a wiki made together, with a card and a page.
pub(crate) struct Made {
    pub(crate) board: String,
    pub(crate) card: String,
    pub(crate) wiki: String,
    pub(crate) page: String,
}

impl Made {
    /// The card and the page.
    pub(crate) fn both(&self) -> [&str; 2] {
        [&self.card, &self.page]
    }
}

/// A board and a wiki of the workspace at `at` that `maker` makes, of the visibility
/// `visibility` (`private` for the invited only, `open` for everybody in the workspace), with a
/// card and a page called `title`.
pub(crate) async fn made(maker: &Client, at: &str, visibility: &str, title: &str) -> Made {
    let id_of = |response: ApiResponse| {
        response.check_created();
        response.value()["id"].as_str().expect("an id").to_owned()
    };
    let named = json!({"visibility": visibility, "title": format!("{title} (its container)")});
    let board = id_of(maker.post(&format!("{at}/boards"), &named).await);
    let wiki = id_of(maker.post(&format!("{at}/wikis"), &named).await);
    let titled = json!({"title": title});
    let card = id_of(
        maker
            .post(&format!("{at}/boards/{board}/cards"), &titled)
            .await,
    );
    let page = id_of(
        maker
            .post(&format!("{at}/wikis/{wiki}/pages"), &titled)
            .await,
    );
    Made {
        board,
        card,
        wiki,
        page,
    }
}

/// Asks what the references to `ids` name, as `client`, under the workspace at `at`.
pub(crate) async fn resolved(client: &Client, at: &str, ids: &[&str]) -> ApiResponse {
    client.post(&format!("{at}/references/resolve"), &ids).await
}

/// The identifiers of what the answer names, in its order.
pub(crate) fn named(response: &ApiResponse) -> Vec<String> {
    response.check_ok();
    response
        .value()
        .as_array()
        .expect("a list")
        .iter()
        .map(|linked| linked["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// Checks that `client` is answered about `ids`, alone and beside `known` (which it
/// reads), exactly as about identifiers of nothing: the same bytes, and no title that
/// is hidden.
pub(crate) async fn check_as_nothing(
    client: &Client,
    at: &str,
    ids: &[&str],
    known: &str,
    what: &str,
) {
    let nothing: Vec<&str> = ids.iter().map(|_| UNKNOWN_ID).collect();
    let (of_ids, of_nothing) = (
        resolved(client, at, ids).await,
        resolved(client, at, &nothing).await,
    );
    of_ids.check_ok();
    assert_eq!(of_ids.text(), "[]", "{what}");
    assert_eq!(of_ids.text(), of_nothing.text(), "{what}");
    for id in ids {
        let (beside, beside_nothing) = (
            resolved(client, at, &[known, id]).await,
            resolved(client, at, &[known, UNKNOWN_ID]).await,
        );
        assert_eq!(named(&beside), [known], "{what}: {id}");
        assert_eq!(beside.text(), beside_nothing.text(), "{what}: {id}");
        assert!(!beside.text().contains(HIDDEN), "{what}: {}", beside.text());
    }
}

/// Puts `user_id` on the board or wiki as its viewer, as `admin`, who administers it.
pub(crate) async fn let_read(admin: &Client, board_id: &str, user_id: &str) {
    let member = ContainerMember {
        container_id: board_id.to_owned(),
        user_id: user_id.to_owned(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    admin
        .post(&format!("{TEST_WS}/containers/{board_id}/members"), &member)
        .await
        .check_ok();
}

/// A card and a page are told to who reads them, as what they are and where: each once,
/// in the order asked. A block that is neither is told to nobody; a list too long, and
/// what is no identifier, are refused.
#[tokio::test]
async fn a_reference_names_a_card_or_a_page_to_who_reads_it() {
    let th = Setup::native().start().await.init_basic().await;
    let mine = made(&th.client, TEST_WS, "private", "The plan").await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", mine.card),
            &json!({"icon": "📌"}),
        )
        .await
        .check_ok();

    let answer = resolved(&th.client, TEST_WS, &[&mine.page, &mine.card, &mine.page]).await;
    answer.check_ok();
    assert_eq!(
        answer.value(),
        json!([
            {"id": mine.page, "kind": "page", "title": "The plan", "icon": "",
                "containerId": mine.wiki, "containerKind": "wiki"},
            {"id": mine.card, "kind": "card", "title": "The plan", "number": 1, "icon": "📌",
                "containerId": mine.board, "containerKind": "board"},
        ]),
    );
    assert_eq!(resolved(&th.client, TEST_WS, &[]).await.text(), "[]");

    // A comment and the board itself are no card, no page and no view (what a view is
    // told as is board_view_bodies.rs's).
    let others = [Block {
        container_id: mine.board.clone(),
        parent_id: mine.card.clone(),
        block_type: BlockType::Comment,
        title: "neither".to_owned(),
        ..Block::default()
    }];
    let stored = insert(&th.client, &mine.board, &others).await;
    let neither = [&stored[0].id, &mine.board, &mine.wiki];
    let neither: Vec<&str> = neither.iter().map(|id| id.as_str()).collect();
    check_as_nothing(&th.client, TEST_WS, &neither, &mine.card, "neither").await;

    // A hundred at once and no more; and only what has the form of an identifier.
    let hundred: Vec<String> = (0..100).map(|_| fvoci_model::new_id()).collect();
    let mut many: Vec<&str> = hundred.iter().map(String::as_str).collect();
    assert_eq!(resolved(&th.client, TEST_WS, &many).await.text(), "[]");
    many.push(&mine.card);
    resolved(&th.client, TEST_WS, &many)
        .await
        .check_bad_request();
    for no_id in ["", "plan", "00000000-0000-0000-0000-000000000000"] {
        let refused = resolved(&th.client, TEST_WS, &[&mine.card, no_id]).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "/1", "{no_id:?}");
    }
    let upper = mine.card.to_uppercase();
    resolved(&th.client, TEST_WS, &[&upper])
        .await
        .check_bad_request();
    for no_list in [json!({"ids": [mine.card]}), json!(null), json!([1])] {
        th.client
            .post(&format!("{TEST_WS}/references/resolve"), &no_list)
            .await
            .check_bad_request();
    }
    th.teardown().await;
}

/// In the trash a card or a page is told to nobody, and so is what is on a board or a
/// wiki in the trash; each is told again when it is back.
#[tokio::test]
async fn what_is_in_the_trash_is_resolved_as_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let mine = made(&th.client, TEST_WS, "private", "The plan").await;
    trash_card(&th.client, &mine.board, &mine.card)
        .await
        .check_no_content();
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&mine.card],
        &mine.page,
        "in the trash",
    )
    .await;
    restore_card(&th.client, &mine.board, &mine.card)
        .await
        .check_ok();
    assert_eq!(
        named(&resolved(&th.client, TEST_WS, &[&mine.card]).await),
        [mine.card.as_str()]
    );

    // So is a page; and with its board or its wiki in the trash, what is on it.
    let page_path = format!("{TEST_WS}/wikis/{}/pages/{}", mine.wiki, mine.page);
    th.client.delete(&page_path).await.check_no_content();
    check_as_nothing(
        &th.client,
        TEST_WS,
        &[&mine.page],
        &mine.card,
        "a page gone",
    )
    .await;
    let other = made(&th.client, TEST_WS, "private", HIDDEN).await;
    for container in [&other.board, &other.wiki] {
        th.client
            .delete(&format!("{TEST_WS}/containers/{container}"))
            .await
            .check_no_content();
    }
    check_as_nothing(
        &th.client,
        TEST_WS,
        &other.both(),
        &mine.card,
        "boards gone",
    )
    .await;
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/restore", other.wiki))
        .await
        .check_ok();
    assert_eq!(
        named(&resolved(&th.client, TEST_WS, &other.both()).await),
        [other.page.as_str()]
    );
    th.teardown().await;
}

/// To a member of the workspace, what is for the invited only is nothing until they
/// are invited, whereas an open board and an open wiki are both read without joining.
#[tokio::test]
async fn what_a_member_is_not_on_is_resolved_as_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let own = made(&th.client2, TEST_WS, "private", "own").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let open = made(&th.client, TEST_WS, "open", "open-title").await;
    let member = &th.client2;
    let member_id = member.me().await.id;

    let hidden = [private.card.as_str(), &private.page];
    check_as_nothing(member, TEST_WS, &hidden, &own.card, "no member").await;
    assert_eq!(
        named(&resolved(member, TEST_WS, &open.both()).await),
        [open.card.as_str(), open.page.as_str()],
        "open board card and open wiki page are resolved without joining"
    );

    // Joined, and invited: each is told from then on, and the other stays nothing.
    member
        .post(
            &format!("{TEST_WS}/containers/{}/join", open.board),
            &json!({}),
        )
        .await
        .check_ok();
    let_read(&th.client, &private.wiki, &member_id).await;
    let asked = [private.both(), open.both()].concat();
    assert_eq!(
        named(&resolved(member, TEST_WS, &asked).await),
        [private.page.as_str(), &open.card, &open.page]
    );
    // What stays nothing is nothing wherever it is among what is asked.
    let other = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let still = [private.card.as_str(), other.card.as_str()];
    for ids in [still, [still[1], still[0]]] {
        let known = [own.card.as_str(), ids[0], ids[1]];
        let unknown = [own.card.as_str(), UNKNOWN_ID, UNKNOWN_ID];
        assert_eq!(
            resolved(member, TEST_WS, &known).await.text(),
            resolved(member, TEST_WS, &unknown).await.text()
        );
    }

    // Taken off again, it is nothing again.
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{member_id}",
            private.wiki
        ))
        .await
        .check_no_content();
    check_as_nothing(member, TEST_WS, &[&private.page], &own.card, "taken off").await;
    th.teardown().await;
}

/// A guest of the workspace is told what is on the boards and wikis they were invited
/// to, and nothing of what is for everybody who works there: an open wiki among it.
#[tokio::test]
async fn a_guest_is_told_what_it_was_invited_to_and_nothing_else() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = crate::guests::guest_of_the_workspace(&th).await;
    let invited = made(&th.client, TEST_WS, "private", "for the guest").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let open = made(&th.client, TEST_WS, "open", HIDDEN).await;
    let_read(&th.client, &invited.board, &guest).await;

    assert_eq!(
        named(&resolved(&th.client2, TEST_WS, &invited.both()).await),
        [invited.card.as_str()]
    );
    // A template for everybody who works in the workspace is read by its members, and
    // is not for a guest.
    let template: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..crate::boards::new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, HIDDEN)
        })
        .await
        .created_json();
    let on_template = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", template.id),
            &json!({"title": HIDDEN}),
        )
        .await
        .value();
    let on_template = on_template["id"].as_str().expect("an id");
    let member = th.add_user("member.here").await;
    assert_eq!(
        named(&resolved(&member, TEST_WS, &[on_template]).await),
        [on_template]
    );

    let mut hidden = [private.both(), open.both(), [&invited.page, UNKNOWN_ID]].concat();
    hidden.push(on_template);
    check_as_nothing(&th.client2, TEST_WS, &hidden, &invited.card, "a guest").await;
    th.teardown().await;
}

/// An administrator of the workspace who only manages what is for the invited only
/// sees that it is there and is told nothing that is on it, until they are on it.
#[tokio::test]
async fn who_only_manages_is_told_nothing_of_what_is_on_it() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = crate::workspace_admin::workspace_admin(&th).await;
    let own = made(&th.client2, TEST_WS, "private", "own").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let managed: Vec<Container> = th.client2.get(&format!("{TEST_WS}/boards")).await.ok_json();
    assert!(
        managed.iter().any(|board| board.id == private.board),
        "the administrator sees the board"
    );

    check_as_nothing(&th.client2, TEST_WS, &private.both(), &own.card, "managing").await;
    let_read(&th.client2, &private.board, &admin).await;
    assert_eq!(
        named(&resolved(&th.client2, TEST_WS, &private.both()).await),
        [private.card.as_str()]
    );
    th.teardown().await;
}

/// What is of another workspace is nothing under this one, for who reads it there too:
/// a reference stays in its workspace.
#[tokio::test]
async fn what_is_of_another_workspace_is_resolved_as_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let here = made(&th.client, TEST_WS, "open", "known here").await;
    let test_made = made(&th.client, TEST_WS, "open", HIDDEN).await;
    let there = made(&th.client, ROOT_WS, "open", "known there").await;
    let root_made = made(&th.client, ROOT_WS, "open", HIDDEN).await;

    let (of_test, of_root) = (test_made.both(), root_made.both());
    check_as_nothing(&th.client, TEST_WS, &of_root, &here.card, "of the root").await;
    check_as_nothing(&th.client, ROOT_WS, &of_test, &there.card, "of the test").await;
    assert_eq!(
        named(&resolved(&th.client, ROOT_WS, &of_root).await),
        of_root
    );
    // Under a workspace the user is not in, the workspace is not there.
    let stranger = th.add_user("stranger.here").await;
    let hidden = format!("/w/{}", crate::common::HIDDEN_WORKSPACE_ID);
    for (client, at) in [(&th.client, hidden.as_str()), (&stranger, hidden.as_str())] {
        resolved(client, at, &here.both()).await.check_not_found();
    }
    th.teardown().await;
}

/// What is of another organization is nothing, under whichever workspace it is asked.
#[tokio::test]
async fn what_is_of_another_organization_is_resolved_as_nothing() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let here = made(&th.client, TEST_WS, "open", "here").await;
    let (theirs, their_workspace, their_board) = crate::workspaces::another_organization(&th).await;
    let members = theirs.get_members_for_container(&their_board).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut card = Block {
        id: fvoci_model::new_id(),
        container_id: their_board.clone(),
        block_type: BlockType::Card,
        title: HIDDEN.to_owned(),
        ..Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");
    let mut wiki = Container {
        id: fvoci_model::new_id(),
        workspace_id: their_workspace.clone(),
        visibility: ContainerVisibility::Open,
        kind: ContainerKind::Wiki,
        ..Container::default()
    };
    theirs
        .insert_container_with_admin(&mut wiki, &owner)
        .await
        .expect("their wiki is stored");
    let new_page = NewPage {
        title: HIDDEN.to_owned(),
        ..NewPage::default()
    };
    let page = theirs
        .create_page(&wiki.id, &new_page, PageLimits::default(), &owner)
        .await
        .expect("their page is stored")
        .first;

    let their_ids = [card.id.as_str(), page.id.as_str(), their_board.as_str()];
    check_as_nothing(&th.client, TEST_WS, &their_ids, &here.card, "another's").await;
    resolved(&th.client, &format!("/w/{their_workspace}"), &their_ids)
        .await
        .check_not_found();
    th.teardown().await;
}

/// A template the server ships with is every workspace's to read and of none: a
/// reference shows nothing of it.
#[tokio::test]
async fn what_is_on_a_built_in_template_is_resolved_as_nothing() {
    let th = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let here = made(&th.client, TEST_WS, "open", "here").await;
    let templates: Vec<Container> = th
        .client
        .get(&format!("{TEST_WS}/templates"))
        .await
        .ok_json();
    let template = templates
        .iter()
        .find(|board| board.is_builtin)
        .expect("a template the server ships with");
    let cards: Vec<Value> = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", template.id))
        .await
        .ok_json();
    let card = cards[0]["id"].as_str().expect("a card of the template");
    th.client
        .get(&format!("{TEST_WS}/cards/{card}"))
        .await
        .check_ok();

    check_as_nothing(&th.client, TEST_WS, &[card], &here.card, "shipped").await;
    th.teardown().await;
}

/// Without a session nothing is asked, whatever link to a board the request shows: the
/// operation takes no read token, and there is none like it for a shared board.
#[tokio::test]
async fn a_reader_by_a_link_does_not_ask_what_references_name() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = made(&th.client, TEST_WS, "open", HIDDEN).await;
    let token = th.share(&shared.board).await;
    let ids = [shared.card.as_str()];
    let reader = th.anonymous();
    for (path, status) in [
        (
            format!("{TEST_WS}/references/resolve"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!("{TEST_WS}/references/resolve?readToken={token}"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!(
                "/shared/{}/references/resolve?readToken={token}",
                shared.board
            ),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let refused = reader.post(&path, &ids).await;
        assert_eq!(refused.status, status, "{path}");
        assert!(!refused.text().contains(HIDDEN), "{path}");
    }
    th.teardown().await;
}

/// How many reads of the store a question takes says how many identifiers it asked
/// about and nothing of what they name: whether there is such a block, whether the
/// asker reads it, or which of them are on one board.
#[tokio::test]
async fn the_reads_a_question_takes_tell_nothing_of_what_is_named() {
    let th = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let member = &th.client2;
    let own = made(member, TEST_WS, "private", "own").await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let other = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let beside = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", private.board),
            &json!({"title": HIDDEN}),
        )
        .await
        .value();
    let beside = beside["id"].as_str().expect("an id");
    let (nothing, nothing_else) = (fvoci_model::new_id(), fvoci_model::new_id());

    let pairs: [[&str; 2]; 8] = [
        [&own.card, &own.page],
        [&own.card, &private.card],
        // Two cards of one board the member does not read, and one with its board.
        [&private.card, beside],
        [&private.card, &private.board],
        [&private.card, &other.board],
        [&private.card, &other.page],
        [&private.card, &nothing],
        [&nothing, &nothing_else],
    ];
    let mut taken = Vec::new();
    for pair in pairs {
        taken.push(reads_of(&th, member, &pair).await);
    }
    assert!(
        taken.iter().all(|reads| *reads == taken[0]),
        "every pair takes the same reads: {taken:?}"
    );
    let of_one = reads_of(&th, member, &[&private.card]).await;
    assert_eq!(of_one, reads_of(&th, member, &[&nothing]).await);
    assert!(of_one < taken[0], "one identifier takes fewer than two");
    th.teardown().await;
}

/// How many reads the store counted for one question about `ids`.
async fn reads_of(th: &TestServer, client: &Client, ids: &[&str]) -> u64 {
    fewest_reads(th, async || resolved(client, TEST_WS, ids).await.check_ok()).await
}

/// How many reads the store counted while `ask` asked once: the fewest of a few
/// askings, as what the server does on its own between two scrapes reads too.
pub(crate) async fn fewest_reads(th: &TestServer, ask: impl AsyncFn()) -> u64 {
    let counted = async || {
        let text = crate::metrics::scrape(th).await;
        text.lines()
            .filter(|line| line.starts_with("fvoci_db_call_duration_seconds_count{access=\"read\""))
            .filter_map(|line| line.rsplit(' ').next()?.parse::<u64>().ok())
            .sum::<u64>()
    };
    let mut fewest = u64::MAX;
    for _ in 0..3 {
        let before = counted().await;
        ask().await;
        fewest = fewest.min(counted().await - before);
    }
    fewest
}
