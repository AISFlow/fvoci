//! Who finds what: one world of cards and pages that some users read and others do
//! not, every one of them holding the word searched for, searched the two ways a
//! server searches. Through a search server whose index is wrong in every way (it
//! answers every search with all of them), and in the database of a server without
//! one: both answer each user with the same, what they read now and nothing else.

use fvoci_model::{Block, BlockType, Container, ContainerVisibility, SearchHit, new_id};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use serde_json::json;

use crate::boards::{delete_member, editor, member, new_board};
use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID,
    TEST_WS, TestServer, UNKNOWN_ID,
};
use crate::content_search::{
    found, new_card, plant_claiming, private_board, search, set_role, sorted, started, stored_card,
};
use crate::fake_search::FakeSearch;
use crate::search_pages::{new_page, new_wiki};
use crate::workspaces::another_organization;

/// The word every card and page of the world holds.
const WORD: &str = "환불";

/// A private board that `user_id` reads through a team and is no member of, and a
/// card of it: the board's and the card's identifiers.
async fn card_read_through_a_team(th: &TestServer, user_id: &str) -> (String, String) {
    let board = private_board(&th.client, "Teamed").await;
    let team: fvoci_model::Team = th
        .client
        .post(&format!("{TEST_WS}/teams"), &json!({"name": "Support"}))
        .await
        .ok_json();
    th.client
        .put(
            &format!("{TEST_WS}/teams/{}/members/{user_id}", team.id),
            &json!({}),
        )
        .await
        .check_no_content();
    th.client
        .put(
            &format!("{TEST_WS}/containers/{}/teams/{}", board.id, team.id),
            &json!({"role": "viewer"}),
        )
        .await
        .check_no_content();
    let card = new_card(&th.client, &board.id, "팀의 카드", WORD).await;
    (board.id, card.id)
}

/// A board that went to the trash whole, and a card of it.
async fn card_of_a_board_in_the_trash(th: &TestServer) -> (String, String) {
    let board = private_board(&th.client, "Gone").await;
    let card = new_card(&th.client, &board.id, "지운 보드의 카드", WORD).await;
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    (board.id, card.id)
}

/// Two cards of a board that were deleted: one in the trash, and one that went from
/// there for good.
async fn cards_that_went(th: &TestServer, board_id: &str) -> (String, String) {
    let trashed = new_card(&th.client, board_id, "지운 카드", WORD).await;
    let purged = new_card(&th.client, board_id, "사라진 카드", WORD).await;
    for card in [&trashed, &purged] {
        th.client
            .delete(&format!("{TEST_WS}/boards/{board_id}/blocks/{}", card.id))
            .await
            .check_no_content();
    }
    // The other card of the trash stays: only the one is removed for good.
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM deleted_blocks WHERE id = '{}'",
            purged.id
        ))
        .await
        .expect("the card is gone for good");
    (trashed.id, purged.id)
}

/// What is stored and has no document for a searcher of the test workspace: a card of
/// a workspace they are not in, a card of a template, a card template of the board
/// `open`, and a comment on `card_id` of it.
async fn blocks_without_a_document(
    th: &TestServer,
    open: &Container,
    card_id: &str,
    author: &str,
) -> Vec<String> {
    let store = th.app.store();
    let hidden_owner = th.stranger_in(HIDDEN_WORKSPACE_ID).await;
    let hidden = crate::boards::board_with_admin(
        th,
        new_board(HIDDEN_WORKSPACE_ID, ContainerVisibility::Open, "Hidden"),
        &hidden_owner,
    )
    .await;
    let hidden_card = stored_card(store, &hidden.id, "다른 워크스페이스의 환불").await;
    let template: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "Template")
        })
        .await
        .created_json();
    let template_card = new_card(&th.client, &template.id, "템플릿 카드", WORD).await;
    let card_template: fvoci_model::Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", open.id),
            &json!({"title": "환불 카드 템플릿", "isTemplate": true}),
        )
        .await
        .created_json();
    let mut comment = Block {
        id: new_id(),
        container_id: open.id.clone(),
        parent_id: card_id.to_owned(),
        block_type: BlockType::Comment,
        title: "환불 댓글".to_owned(),
        ..Block::default()
    };
    store
        .insert_block(&mut comment, author)
        .await
        .expect("the comment is stored");
    vec![hidden_card, template_card.id, card_template.id, comment.id]
}

/// Pages that went to the trash: one of a wiki that stays, and the wiki of another,
/// whole. Their identifiers, and the wikis they were in.
async fn pages_that_went(th: &TestServer) -> Vec<(String, String)> {
    let kept = new_wiki(&th.client).await;
    let trashed = new_page(&th.client, &kept, "", "지운 쪽", WORD).await;
    th.client
        .delete(&format!("{TEST_WS}/wikis/{kept}/pages/{}", trashed.id))
        .await
        .check_no_content();
    let gone = new_wiki(&th.client).await;
    let of_gone = new_page(&th.client, &gone, "", "지운 위키의 쪽", WORD).await;
    th.client
        .delete(&format!("{TEST_WS}/containers/{gone}"))
        .await
        .check_no_content();
    vec![(trashed.id, kept), (of_gone.id, gone)]
}

/// The pages of two wikis: one open to the workspace that the second user has not
/// joined, and a private one of the first user alone.
async fn pages_of_wikis(th: &TestServer) -> (String, String, String, String) {
    let open = th
        .client
        .post(&format!("{TEST_WS}/wikis"), &json!({"visibility": "open"}))
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let open_page = new_page(&th.client, &open, "", "열린 위키의 쪽", WORD).await;
    let private = new_wiki(&th.client).await;
    let private_page = new_page(&th.client, &private, "", "비공개 위키의 쪽", WORD).await;
    (open, open_page.id, private, private_page.id)
}

/// Who searches, and what each is to find.
struct World {
    guest: Client,
    /// What the index claims, of what: the identifier and the container it names.
    planted: Vec<(String, String)>,
    /// What the first user, the second, and the guest read.
    admin_reads: Vec<String>,
    member_reads: Vec<String>,
    guest_reads: Vec<String>,
    /// The private card, and its board.
    private: (String, String),
}

/// Makes the world on `th`.
#[expect(
    clippy::too_many_lines,
    reason = "every way of not reading a card or a page, side by side"
)]
async fn world(th: &TestServer) -> World {
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    let guest = th.add_user("guest").await;
    let guest_id = guest.me().await.id;
    set_role(th, &guest_id, "guest").await;
    let stranger = th.stranger_in(TEST_WORKSPACE_ID).await;
    let store = th.app.store();
    // The first user administers the test workspace: they manage every board of it,
    // and read those they have a role on.
    set_role(th, &user1, "admin").await;
    // An empty board of the first user in the root workspace: a search there asks.
    th.create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    // An open board, and a card of it that every member who joined reads.
    let open: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open",
        ))
        .await
        .created_json();
    let open_card = new_card(&th.client, &open.id, "열린 카드", "환불규정을 따른다").await;
    th.client2
        .post_empty(&format!("{TEST_WS}/containers/{}/join", open.id))
        .await
        .check_ok();
    // An open board the second user has not joined: theirs to join, and not to read.
    let unjoined: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Unjoined",
        ))
        .await
        .created_json();
    let unjoined_card = new_card(&th.client, &unjoined.id, "남의 열린 카드", WORD).await;
    // A private board of the first user alone.
    let private = private_board(&th.client, "Private").await;
    let private_card = new_card(&th.client, &private.id, "비공개 카드", WORD).await;
    // A private board the first user, an administrator of the workspace, is not on.
    let managed = crate::boards::board_with_admin(
        th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Managed"),
        &stranger,
    )
    .await;
    let managed_card = stored_card(store, &managed.id, "남의 카드의 환불").await;
    // A private board the guest was invited to.
    let invited = private_board(&th.client, "Invited").await;
    th.client
        .add_member(&member(
            &invited.id,
            &guest_id,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    let invited_card = new_card(&th.client, &invited.id, "초대된 카드", WORD).await;
    // A private board the second user was on, and is not now.
    let left = private_board(&th.client, "Left").await;
    th.client
        .add_member(&editor(&left.id, &user2))
        .await
        .check_ok();
    let left_card = new_card(&th.client, &left.id, "떠난 카드", WORD).await;
    delete_member(&th.client, &left.id, &user2)
        .await
        .check_no_content();
    let (teamed, teamed_card) = card_read_through_a_team(th, &user2).await;
    let (gone, gone_card) = card_of_a_board_in_the_trash(th).await;
    let (trashed, purged) = cards_that_went(th, &open.id).await;
    let no_documents = blocks_without_a_document(th, &open, &open_card.id, &user1).await;
    let (open_wiki, open_page, private_wiki, private_page) = pages_of_wikis(th).await;
    let pages_gone = pages_that_went(th).await;

    let mut planted = vec![
        (open_card.id.clone(), open.id.clone()),
        // Twice, and once more as of a board it is not on.
        (open_card.id.clone(), open.id.clone()),
        (unjoined_card.id.clone(), unjoined.id.clone()),
        (private_card.id.clone(), private.id.clone()),
        (private_card.id.clone(), open.id.clone()),
        (managed_card.clone(), managed.id.clone()),
        (managed_card.clone(), open.id.clone()),
        (invited_card.id.clone(), invited.id.clone()),
        (left_card.id.clone(), left.id.clone()),
        (teamed_card.clone(), teamed),
        (gone_card.clone(), gone),
        (gone_card, open.id.clone()),
        (trashed, open.id.clone()),
        (purged, open.id.clone()),
        (open_page.clone(), open_wiki),
        (private_page.clone(), private_wiki),
        (UNKNOWN_ID.to_owned(), open.id.clone()),
        ("not-an-identifier".to_owned(), open.id.clone()),
        ("' OR 1=1 --".to_owned(), open.id.clone()),
    ];
    planted.extend(no_documents.into_iter().map(|id| (id, open.id.clone())));
    planted.extend(pages_gone);
    // Another organization's card, where the database holds more than one.
    if db_type_of_the_tests() == DbType::Postgres {
        let (theirs, _, their_board) = another_organization(th).await;
        let their_card = stored_card(&theirs, &their_board, "남의 조직의 환불").await;
        planted.push((their_card, open.id.clone()));
    }
    World {
        guest,
        planted,
        admin_reads: sorted(&[
            &open_card.id,
            &unjoined_card.id,
            &private_card.id,
            &invited_card.id,
            &left_card.id,
            &teamed_card,
            &open_page,
            &private_page,
        ]),
        member_reads: sorted(&[&open_card.id, &unjoined_card.id, &teamed_card, &open_page]),
        guest_reads: sorted(&[&invited_card.id]),
        private: (private_card.id, private.id),
    }
}

/// What each user found.
struct Answers {
    admin: ApiResponse,
    member: ApiResponse,
    guest: ApiResponse,
    in_root: ApiResponse,
}

/// Searches the world as each user.
async fn ask(th: &TestServer, world: &World) -> Answers {
    Answers {
        admin: search(&th.client, TEST_WS, WORD).await,
        member: search(&th.client2, TEST_WS, WORD).await,
        guest: search(&world.guest, TEST_WS, WORD).await,
        in_root: search(&th.client, ROOT_WS, WORD).await,
    }
}

/// Sees that each user found what they read, and nothing else.
fn each_found_what_they_read(world: &World, answers: &Answers) {
    assert_eq!(
        found(&answers.admin),
        world.admin_reads,
        "the first user reads their boards and wikis, and no board they only manage"
    );
    assert_eq!(
        found(&answers.member),
        world.member_reads,
        "the second user reads the open board they joined, a team's board and the open \
         wiki, and no board they left"
    );
    assert_eq!(
        found(&answers.guest),
        world.guest_reads,
        "a guest reads the board they were invited to, and no open board or wiki"
    );
    assert_eq!(
        found(&answers.in_root),
        Vec::<String>::new(),
        "nothing of another workspace"
    );
    // The hits say what the database says: the board the card is on, not the one an
    // index claimed.
    let hits: Vec<SearchHit> = answers.admin.ok_json();
    let (card, board) = &world.private;
    let hit = hits.iter().find(|hit| &hit.id == card).expect("found");
    assert_eq!(&hit.container_id, board, "the board the database says");
    assert_eq!(hit.title, "비공개 카드", "the title the database says");
}

#[tokio::test]
async fn a_stale_index_shows_nobody_what_they_do_not_read_now() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let world = world(&th).await;
    for (id, board_id) in &world.planted {
        plant_claiming(&th, &fake, id, board_id);
    }

    let answers = ask(&th, &world).await;
    let asked = fake.searches().len();
    th.teardown().await;

    each_found_what_they_read(&world, &answers);
    assert_eq!(asked, 4, "the search server was asked every time");
}

#[tokio::test]
async fn the_database_shows_nobody_what_they_do_not_read() {
    let th = Setup::native().start().await.init_basic().await;
    let world = world(&th).await;

    let answers = ask(&th, &world).await;
    th.teardown().await;

    each_found_what_they_read(&world, &answers);
}

/// Sets up multiple boards in the test workspace with cards numbered `#1` and `#2`,
/// including a private board, an unjoined open board, a team-accessible board, a
/// guest-invited board, a template converted from a numbered card, a trashed card,
/// and a non-digit text card holding `#2abc`.
struct NumberedWorld {
    guest: Client,
    alpha_card_2: String,
    beta_card_2: String,
    unjoined_card_2: String,
    team_card_2: String,
    invited_card_2: String,
    text_card: String,
    page_with_hash_2: String,
    alpha_title: String,
    beta_title: String,
}

async fn numbered_world(th: &TestServer) -> NumberedWorld {
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    set_role(th, &user1, "admin").await;
    let guest = th.add_user("num-guest").await;
    let guest_id = guest.me().await.id;
    set_role(th, &guest_id, "guest").await;

    let alpha_title = "Alpha Board".to_owned();
    let beta_title = "Beta Board".to_owned();
    let alpha = private_board(&th.client, &alpha_title).await;
    let beta: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            &beta_title,
        ))
        .await
        .created_json();
    th.client2
        .post_empty(&format!("{TEST_WS}/containers/{}/join", beta.id))
        .await
        .check_ok();
    let unjoined: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Unjoined Board",
        ))
        .await
        .created_json();
    let invited = private_board(&th.client, "Guest Board").await;
    th.client
        .add_member(&member(
            &invited.id,
            &guest_id,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    let (teamed, _) = card_read_through_a_team(th, &user2).await;

    // Board Alpha: #1 (ordinary), #2 (ordinary), #3 (contains "#2abc" in body)
    let alpha_1 = new_card(&th.client, &alpha.id, "Alpha first", "body").await;
    let alpha_2 = new_card(&th.client, &alpha.id, "Alpha second", "body").await;
    let text_card = new_card(&th.client, &alpha.id, "Release note", "see #2abc details").await;
    // Convert Alpha #1 into a template: DB preserves number 1, search must exclude it.
    th.client
        .patch(
            &format!("{TEST_WS}/boards/{}/blocks/{}", alpha.id, alpha_1.id),
            &json!({"updatedFields": {"isTemplate": true}}),
        )
        .await
        .check_no_content();

    // Board Beta: #1 (ordinary), #2 (ordinary)
    new_card(&th.client, &beta.id, "Beta first", "body").await;
    let beta_2 = new_card(&th.client, &beta.id, "Beta second", "body").await;

    // Unjoined open board: #1, #2 (user2 did NOT join; must be hidden from user2)
    new_card(&th.client, &unjoined.id, "Unjoined first", "body").await;
    let unjoined_2 = new_card(&th.client, &unjoined.id, "Unjoined second", "body").await;

    // Guest-invited private board: #1, #2
    new_card(&th.client, &invited.id, "Guest first", "body").await;
    let invited_2 = new_card(&th.client, &invited.id, "Guest second", "body").await;

    // Team-shared private board already has #1 from helper; add #2.
    let team_2 = new_card(&th.client, &teamed, "Team second", "body").await;

    // Trashed board with #2: excluded.
    let trashed = private_board(&th.client, "Trashed Board").await;
    new_card(&th.client, &trashed.id, "Trashed first", "body").await;
    new_card(&th.client, &trashed.id, "Trashed second", "body").await;
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", trashed.id))
        .await
        .check_no_content();

    // Wiki page mentioning `#2` in its body: returned by `q=#2&kind=page`, never by `q=#2` card number search.
    let notes_wiki = new_wiki(&th.client).await;
    let page_with_hash_2 = new_page(&th.client, &notes_wiki, "", "Sprint notes", "see #2 notes")
        .await
        .id;

    // Another organization's #2 card on Postgres: excluded.
    if db_type_of_the_tests() == DbType::Postgres {
        let (theirs, _, their_board) = another_organization(th).await;
        stored_card(&theirs, &their_board, "Other org 1").await;
        stored_card(&theirs, &their_board, "Other org 2").await;
    }

    NumberedWorld {
        guest,
        alpha_card_2: alpha_2.id,
        beta_card_2: beta_2.id,
        unjoined_card_2: unjoined_2.id,
        team_card_2: team_2.id,
        invited_card_2: invited_2.id,
        text_card: text_card.id,
        page_with_hash_2,
        alpha_title,
        beta_title,
    }
}

async fn check_numbered_search(th: &TestServer, w: &NumberedWorld) {
    // `#2` and `#0002` normalize to 2 and return cards across every readable board,
    // with `number: Some(2)` and `containerTitle` populated.
    for query in ["#2", "#0002"] {
        let member_hits: Vec<SearchHit> = search(&th.client2, TEST_WS, query).await.ok_json();
        assert_eq!(
            sorted(
                &member_hits
                    .iter()
                    .map(|hit| hit.id.as_str())
                    .collect::<Vec<_>>()
            ),
            sorted(&[&w.beta_card_2, &w.unjoined_card_2, &w.team_card_2]),
            "member finds #2 on open boards (joined or unjoined) and team board ({query})"
        );
        for hit in &member_hits {
            assert_eq!(hit.number, Some(2), "hit reports card number 2");
            assert!(!hit.container_title.is_empty(), "{hit:?}");
        }
        let beta_hit = member_hits
            .iter()
            .find(|hit| hit.id == w.beta_card_2)
            .expect("beta hit");
        assert_eq!(
            beta_hit.container_title, w.beta_title,
            "beta hit reports its board title"
        );

        let guest_hits: Vec<SearchHit> = search(&w.guest, TEST_WS, query).await.ok_json();
        assert_eq!(
            guest_hits
                .iter()
                .map(|hit| hit.id.as_str())
                .collect::<Vec<_>>(),
            [w.invited_card_2.as_str()],
            "guest finds #2 only on invited board ({query})"
        );
    }

    let owner_hits: Vec<SearchHit> = search(&th.client, TEST_WS, "#2").await.ok_json();
    assert_eq!(
        sorted(
            &owner_hits
                .iter()
                .map(|hit| hit.id.as_str())
                .collect::<Vec<_>>()
        ),
        sorted(&[
            &w.alpha_card_2,
            &w.beta_card_2,
            &w.unjoined_card_2,
            &w.invited_card_2,
            &w.team_card_2,
        ]),
        "owner finds only live in-org readable #2 cards (excluding trashed and other-org boards)"
    );
    let alpha_hit = owner_hits
        .iter()
        .find(|hit| hit.id == w.alpha_card_2)
        .expect("owner finds Alpha #2");
    assert_eq!(
        alpha_hit.container_title, w.alpha_title,
        "alpha hit reports its board title"
    );
    assert_eq!(alpha_hit.number, Some(2), "alpha hit reports card number 2");

    // Converted template (Alpha #1, preserved DB number 1) must NOT appear in `#1`.
    let owner_ones: Vec<SearchHit> = search(&th.client, TEST_WS, "#1").await.ok_json();
    assert!(
        owner_ones.iter().all(|hit| hit.title != "Alpha first"),
        "template with preserved #1 is excluded: {owner_ones:?}"
    );

    // Non-digit characters mixed in (`#2abc`) stay ordinary text search.
    let mixed: Vec<SearchHit> = search(&th.client, TEST_WS, "#2abc").await.ok_json();
    assert_eq!(
        mixed.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
        [w.text_card.as_str()],
        "#2abc stays text search"
    );

    // `kind=page` with `#2` runs text search and returns pages mentioning `#2`, never cards.
    let pages: Vec<SearchHit> =
        crate::content_search::search_with(&th.client, TEST_WS, &[("q", "#2"), ("kind", "page")])
            .await
            .ok_json();
    assert_eq!(
        pages.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
        [w.page_with_hash_2.as_str()],
        "kind=page runs text search for #2 and returns no cards: {pages:?}"
    );
}

#[tokio::test]
async fn hash_number_search_finds_cards_across_boards_with_isolation_via_search_server() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let w = numbered_world(&th).await;
    fvoci_server::search::tend(&th.app).await;

    check_numbered_search(&th, &w).await;
    th.teardown().await;
}

#[tokio::test]
async fn hash_number_search_finds_cards_across_boards_with_isolation_via_database() {
    let th = Setup::native().start().await.init_basic().await;
    let w = numbered_world(&th).await;

    check_numbered_search(&th, &w).await;
    th.teardown().await;
}
