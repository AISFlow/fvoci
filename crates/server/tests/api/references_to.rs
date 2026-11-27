//! What names a card or a page (`getReferencesTo`) and what a reference may name
//! (`searchReferences`): both of what the asker reads and of nothing else. What names
//! from where the asker does not read changes no letter of the answer, and a block the
//! asker does not read is asked about as one that is not there.

use fvoci_model::{Block, BlockType, Container, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{block_path, undelete_path};
use crate::common::{ApiResponse, Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID};
use crate::references::{HIDDEN, Made, fewest_reads, let_read, made};

/// A body that says `text` and names the blocks.
pub(crate) fn body_naming(text: &str, targets: &[&str]) -> Value {
    let mut line = vec![json!({"type": "text", "text": text})];
    line.extend(
        targets
            .iter()
            .map(|target| json!({"type": "reference", "attrs": {"id": target}})),
    );
    json!({"type": "doc", "content": [{"type": "paragraph", "content": line}]})
}

/// Writes the body to the card or the page at `path`, as `client`.
async fn write_body(client: &Client, path: &str, body: &Value) {
    let read_at = client.get(path).await.value()["updatedAt"].clone();
    client
        .patch(path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
}

/// Makes the card of the workspace at `at` name the blocks in its body.
pub(crate) async fn card_names(client: &Client, at: &str, card_id: &str, targets: &[&str]) {
    let body = body_naming("see", targets);
    write_body(client, &format!("{at}/cards/{card_id}"), &body).await;
}

/// Makes the page name the blocks in its body.
pub(crate) async fn page_names(client: &Client, at: &str, made: &Made, targets: &[&str]) {
    let path = format!("{at}/wikis/{}/pages/{}", made.wiki, made.page);
    write_body(client, &path, &body_naming("see", targets)).await;
}

/// A card of the board called `title`, made by `client`: its identifier.
pub(crate) async fn card_of(client: &Client, at: &str, board_id: &str, title: &str) -> String {
    let card = client
        .post(
            &format!("{at}/boards/{board_id}/cards"),
            &json!({"title": title}),
        )
        .await;
    card.check_created();
    card.value()["id"].as_str().expect("an id").to_owned()
}

/// A comment on the card whose body names the blocks: its identifier.
pub(crate) async fn comment_naming(
    client: &Client,
    board_id: &str,
    card_id: &str,
    targets: &[&str],
) -> String {
    let comment = Block {
        container_id: board_id.to_owned(),
        parent_id: card_id.to_owned(),
        block_type: BlockType::Comment,
        body: fvoci_model::Body::parse(body_naming("in a comment", targets))
            .expect("the body may be stored"),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    let stored: Vec<Block> = client
        .insert_blocks(board_id, &[comment])
        .await
        .created_json();
    stored[0].id.clone()
}

/// Asks what names the block, as `client`, with the query `query` (empty, or from `?`).
async fn naming(client: &Client, at: &str, block_id: &str, query: &str) -> ApiResponse {
    client
        .get(&format!("{at}/references/to/{block_id}{query}"))
        .await
}

/// Asks what a reference may name by the words `term`, as `client`.
async fn found(client: &Client, at: &str, term: &str) -> ApiResponse {
    client
        .get(&format!("{at}/references/search?q={term}"))
        .await
}

/// The identifiers of what the answer lists, in its order.
fn listed(response: &ApiResponse) -> Vec<String> {
    response.check_ok();
    response
        .value()
        .as_array()
        .expect("a list")
        .iter()
        .map(|linked| linked["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// A card and a page name each other, and a comment names for the card it is on: each
/// is told once, as what it is and where, the oldest first, in pages. What a block
/// says of itself is not told, nor what is in the trash or names no more.
#[tokio::test]
async fn what_names_a_card_or_a_page_is_told_once_and_in_pages() {
    let th = Setup::native().start().await.init_basic().await;
    let user = &th.client;
    let mine = made(user, TEST_WS, "private", "The plan").await;
    let first = card_of(user, TEST_WS, &mine.board, "first").await;
    let second = card_of(user, TEST_WS, &mine.board, "second").await;
    let (target, page) = (mine.card.as_str(), mine.page.as_str());

    assert_eq!(naming(user, TEST_WS, target, "").await.text(), "[]");
    // The card names itself and so does a comment on it: neither is told.
    card_names(user, TEST_WS, target, &[target, page]).await;
    comment_naming(user, &mine.board, target, &[target]).await;
    assert_eq!(naming(user, TEST_WS, target, "").await.text(), "[]");

    // A card names it twice, in its body and in a comment; another in a comment alone;
    // the page in its body.
    card_names(user, TEST_WS, &first, &[target, target]).await;
    comment_naming(user, &mine.board, &first, &[target]).await;
    let comment = comment_naming(user, &mine.board, &second, &[target, page]).await;
    page_names(user, TEST_WS, &mine, &[target]).await;
    let told = naming(user, TEST_WS, target, "").await;
    told.check_ok();
    assert_eq!(
        told.value(),
        json!([
            {"id": page, "kind": "page", "title": "The plan", "icon": "",
                "containerId": mine.wiki, "containerKind": "wiki"},
            {"id": first, "kind": "card", "title": "first", "number": 2, "icon": "",
                "containerId": mine.board, "containerKind": "board"},
            {"id": second, "kind": "card", "title": "second", "number": 3, "icon": "",
                "containerId": mine.board, "containerKind": "board"},
        ]),
    );
    // The page is named by the card and by the comment's card.
    assert_eq!(
        listed(&naming(user, TEST_WS, page, "").await),
        [target, second.as_str()]
    );

    // In pages: the order is cut, nothing is told twice, and past the end is nothing.
    for (query, expected) in [
        ("?perPage=2", vec![page, first.as_str()]),
        ("?perPage=2&page=1", vec![second.as_str()]),
        ("?perPage=2&page=2", vec![]),
        ("?perPage=1000", vec![page, first.as_str(), second.as_str()]),
    ] {
        let told = naming(user, TEST_WS, target, query).await;
        assert_eq!(listed(&told), expected, "{query}");
    }
    for query in [
        "?page=first",
        "?perPage=many",
        "?page=-1",
        "?perPage=0",
        "?perPage=1001",
    ] {
        naming(user, TEST_WS, target, query)
            .await
            .check_bad_request();
    }

    // A comment in the trash names nothing, nor does a card there; back, they do.
    for (gone, without) in [(&comment, &second), (&first, &first)] {
        user.delete(&block_path(&mine.board, gone))
            .await
            .check_no_content();
        let told = listed(&naming(user, TEST_WS, target, "").await);
        assert!(!told.contains(without), "{gone} is in the trash: {told:?}");
        user.post_empty(&undelete_path(&mine.board, gone))
            .await
            .check_ok();
        assert_eq!(listed(&naming(user, TEST_WS, target, "").await).len(), 3);
    }
    // A body that names it no more is told no more.
    page_names(user, TEST_WS, &mine, &[]).await;
    assert_eq!(
        listed(&naming(user, TEST_WS, target, "").await),
        [first.as_str(), second.as_str()]
    );
    th.teardown().await;
}

/// What names a card from where the asker does not read changes no letter of what
/// they are told, in every page: not for a member who is not on a private board or wiki,
/// while open boards and open wikis are read unjoined and invited boards are told once
/// joined/invited.
#[tokio::test]
async fn what_names_from_where_the_asker_does_not_read_is_not_told() {
    let th = Setup::native().start().await.init_basic().await;
    let (owner, member) = (&th.client, &th.client2);
    let member_id = member.me().await.id;
    let shown = made(owner, TEST_WS, "private", "shown").await;
    let_read(owner, &shown.board, &member_id).await;
    let seen = card_of(owner, TEST_WS, &shown.board, "seen").await;
    card_names(owner, TEST_WS, &seen, &[&shown.card]).await;
    let private = made(owner, TEST_WS, "private", HIDDEN).await;
    let private2 = made(owner, TEST_WS, "private", HIDDEN).await;
    let open = made(owner, TEST_WS, "open", "read unjoined").await;
    let queries = ["", "?perPage=1", "?perPage=1&page=1", "?perPage=1&page=2"];
    // Both a card of an open board and a page of an open wiki are read unjoined.
    card_names(owner, TEST_WS, &open.card, &[&shown.card]).await;
    page_names(owner, TEST_WS, &open, &[&shown.card]).await;

    let mut before = Vec::new();
    for query in queries {
        before.push(naming(member, TEST_WS, &shown.card, query).await.text());
    }
    let mut on_private2 = Vec::new();
    for hidden in [&private, &private2] {
        card_names(owner, TEST_WS, &hidden.card, &[&shown.card]).await;
        if hidden.board == private.board {
            page_names(owner, TEST_WS, hidden, &[&shown.card]).await;
        }
        let another = card_of(owner, TEST_WS, &hidden.board, HIDDEN).await;
        comment_naming(owner, &hidden.board, &another, &[&shown.card]).await;
        on_private2.push(another);
    }
    assert_eq!(
        listed(&naming(owner, TEST_WS, &shown.card, "").await).len(),
        8,
        "the owner is told of all of them"
    );
    for (query, before) in queries.iter().zip(&before) {
        let told = naming(member, TEST_WS, &shown.card, query).await;
        assert_eq!(&told.text(), before, "{query}");
        assert!(!told.text().contains(HIDDEN), "{query}");
    }
    assert_eq!(
        listed(&naming(member, TEST_WS, &shown.card, "").await),
        [seen.as_str(), open.card.as_str(), open.page.as_str()]
    );

    // Invited to private2's board, the member is told of what names it from there, and nothing else.
    let_read(owner, &private2.board, &member_id).await;
    assert_eq!(
        listed(&naming(member, TEST_WS, &shown.card, "").await),
        [
            seen.as_str(),
            &private2.card,
            &open.card,
            &open.page,
            &on_private2[1]
        ]
    );
    th.teardown().await;
}

/// What the asker does not read is asked about as what is not there, to the letter:
/// of a private board/wiki they are not on, in the trash, of another workspace, a block
/// that is neither a card nor a page, and what is no identifier.
#[tokio::test]
async fn what_names_a_block_the_asker_does_not_read_is_asked_as_of_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let (owner, member) = (&th.client, &th.client2);
    let own = made(member, TEST_WS, "private", "own").await;
    let private = made(owner, TEST_WS, "private", HIDDEN).await;
    let open = made(owner, TEST_WS, "open", "read unjoined").await;
    let elsewhere = made(owner, ROOT_WS, "open", HIDDEN).await;
    for target in [&private, &open] {
        card_names(member, TEST_WS, &own.card, &target.both()).await;
        card_names(owner, TEST_WS, &target.card, &target.both()).await;
    }
    let comment = comment_naming(member, &own.board, &own.card, &[&own.card]).await;
    let in_the_trash = card_of(member, TEST_WS, &own.board, "gone").await;
    member
        .delete(&block_path(&own.board, &in_the_trash))
        .await
        .check_no_content();

    let of_nothing = naming(member, TEST_WS, UNKNOWN_ID, "").await;
    of_nothing.check_not_found();
    // Both the open board's card and open wiki's page are read unjoined; private is not.
    naming(member, TEST_WS, &open.card, "").await.check_ok();
    naming(member, TEST_WS, &open.page, "").await.check_ok();
    let mut not_there = [&private.both()[..], &elsewhere.both()].concat();
    not_there.extend([
        comment.as_str(),
        in_the_trash.as_str(),
        own.board.as_str(),
        "plan",
        "00000000-0000-0000-0000-000000000000",
    ]);
    for id in not_there {
        for query in ["", "?perPage=1&page=3"] {
            let asked = naming(member, TEST_WS, id, query).await;
            assert_eq!(asked.problem(), of_nothing.problem(), "{id}{query}");
        }
    }
    th.anonymous()
        .get(&format!("{TEST_WS}/references/to/{}", own.card))
        .await
        .check_unauthorized();
    th.teardown().await;
}

/// A guest is told what names from where it was invited, and an administrator who
/// only manages a private board nothing of what is on it: neither is told a letter more
/// for what names from private containers elsewhere, while the administrator (who works
/// in the workspace) reads open boards and open wikis unjoined, and a guest does not.
#[tokio::test]
async fn a_guest_and_who_only_manages_are_told_what_they_read_names() {
    for administers in [false, true] {
        let th = Setup::native().start().await.init_basic().await;
        let (owner, asker) = (&th.client, &th.client2);
        let asker_id = if administers {
            crate::workspace_admin::workspace_admin(&th).await
        } else {
            crate::guests::guest_of_the_workspace(&th).await
        };
        let shown = made(owner, TEST_WS, "private", "shown").await;
        let_read(owner, &shown.board, &asker_id).await;
        let seen = card_of(owner, TEST_WS, &shown.board, "seen").await;
        card_names(owner, TEST_WS, &seen, &[&shown.card]).await;
        let open = made(owner, TEST_WS, "open", HIDDEN).await;
        card_names(owner, TEST_WS, &open.card, &[&shown.card]).await;
        page_names(owner, TEST_WS, &open, &[&shown.card]).await;
        let before = naming(asker, TEST_WS, &shown.card, "").await.text();

        let private = made(owner, TEST_WS, "private", HIDDEN).await;
        card_names(owner, TEST_WS, &private.card, &[&shown.card]).await;
        page_names(owner, TEST_WS, &private, &[&shown.card]).await;
        let told = naming(asker, TEST_WS, &shown.card, "").await;
        assert_eq!(told.text(), before, "administers: {administers}");
        let mut told_of = vec![seen.as_str()];
        if administers {
            told_of.push(&open.card);
            told_of.push(&open.page);
        }
        assert_eq!(listed(&told), told_of, "administers: {administers}");
        // Nor is private found by its title, nor asked about but as nothing is.
        let mut expected_found = Vec::new();
        if administers {
            expected_found.push(open.page.as_str());
            expected_found.push(open.card.as_str());
        }
        assert_eq!(listed(&found(asker, TEST_WS, HIDDEN).await), expected_found);
        let of_nothing = naming(asker, TEST_WS, UNKNOWN_ID, "").await;
        let mut hidden = [&private.both()[..], &[&shown.page, UNKNOWN_ID]].concat();
        if !administers {
            hidden.push(&open.card);
            hidden.push(&open.page);
        }
        for id in hidden {
            let answer = naming(asker, TEST_WS, id, "").await;
            assert_eq!(answer.problem(), of_nothing.problem(), "{id}");
        }
        th.teardown().await;
    }
}

/// What names from a template for who works in the workspace is told to a member and
/// not to a guest, whom the store lists the template for all the same: to the guest it
/// takes no place in a page, moves none, and costs not one read more.
#[tokio::test]
async fn what_names_from_a_template_a_guest_does_not_read_moves_no_page() {
    let th = Setup::native()
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let guest_id = crate::guests::guest_of_the_workspace(&th).await;
    let (owner, guest) = (&th.client, &th.client2);
    let shown = made(owner, TEST_WS, "private", "shown").await;
    let_read(owner, &shown.board, &guest_id).await;
    let template: Container = owner
        .create_board(&Container {
            is_template: true,
            ..crate::boards::new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, HIDDEN)
        })
        .await
        .created_json();
    // The card of the template is the oldest: it would be the first of the first page.
    let on_template = card_of(owner, TEST_WS, &template.id, HIDDEN).await;
    let first = card_of(owner, TEST_WS, &shown.board, "first").await;
    let second = card_of(owner, TEST_WS, &shown.board, "second").await;
    for card in [&first, &second] {
        card_names(owner, TEST_WS, card, &[&shown.card]).await;
    }
    let pages = async |asker: &Client| {
        let mut pages = Vec::new();
        for page in 0..4 {
            let query = format!("?perPage=1&page={page}");
            pages.push(naming(asker, TEST_WS, &shown.card, &query).await);
        }
        pages
    };
    let texts = |pages: &[ApiResponse]| pages.iter().map(ApiResponse::text).collect::<Vec<_>>();
    let whole = async || naming(guest, TEST_WS, &shown.card, "").await.check_ok();
    let before = pages(guest).await;
    let reads_before = fewest_reads(&th, whole).await;

    card_names(owner, TEST_WS, &on_template, &[&shown.card]).await;
    let after = pages(guest).await;
    assert_eq!(texts(&after), texts(&before));
    let told: Vec<Vec<String>> = after.iter().map(listed).collect();
    assert_eq!(
        told,
        [vec![first.clone()], vec![second.clone()], vec![], vec![]]
    );
    assert_eq!(fewest_reads(&th, whole).await, reads_before);
    assert!(!texts(&after).concat().contains(HIDDEN));

    // Who works in the workspace reads the template, and is told what is on it names.
    let member = th.add_user("member.here").await;
    let_read(owner, &shown.board, &member.me().await.id).await;
    let told: Vec<Vec<String>> = pages(&member).await.iter().map(listed).collect();
    assert_eq!(told, [vec![on_template], vec![first], vec![second], vec![]]);
    th.teardown().await;
}

/// What is of another organization names nothing here and is not found, and is asked
/// about as nothing is.
#[tokio::test]
async fn what_is_of_another_organization_neither_names_nor_is_found() {
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let here = made(&th.client, TEST_WS, "open", "here").await;
    let before = naming(&th.client, TEST_WS, &here.card, "").await.text();
    let (theirs, their_workspace, their_board) = crate::workspaces::another_organization(&th).await;
    let members = theirs.get_members_for_container(&their_board).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut card = Block {
        id: fvoci_model::new_id(),
        container_id: their_board.clone(),
        block_type: BlockType::Card,
        title: HIDDEN.to_owned(),
        body: fvoci_model::Body::parse(body_naming("theirs", &[&here.card]))
            .expect("the body may be stored"),
        ..Block::default()
    };
    theirs
        .insert_block(&mut card, &owner)
        .await
        .expect("their card is stored");

    let told = naming(&th.client, TEST_WS, &here.card, "").await;
    assert_eq!(told.text(), before);
    assert_eq!(found(&th.client, TEST_WS, HIDDEN).await.text(), "[]");
    let of_nothing = naming(&th.client, TEST_WS, UNKNOWN_ID, "").await;
    let of_theirs = naming(&th.client, TEST_WS, &card.id, "").await;
    assert_eq!(of_theirs.problem(), of_nothing.problem());
    for path in [
        format!("/references/to/{}", card.id),
        format!("/references/search?q={HIDDEN}"),
    ] {
        th.client
            .get(&format!("/w/{their_workspace}{path}"))
            .await
            .check_not_found();
    }
    th.teardown().await;
}

/// The cards and pages the asker reads are found by the words of their titles, whatever
/// the case, the one changed last first and twenty at most; without words, the ones
/// changed last. Bodies, the trash and templates are not looked into.
#[tokio::test]
async fn what_a_reference_may_name_is_found_by_its_title() {
    let th = Setup::native().start().await.init_basic().await;
    let user = &th.client;
    let mine = made(user, TEST_WS, "private", "Quarterly Plan").await;
    let percent = card_of(user, TEST_WS, &mine.board, "100% done_now").await;
    let accented = card_of(user, TEST_WS, &mine.board, "ÉTUDE plan").await;
    let other = card_of(user, TEST_WS, &mine.board, "something else").await;
    // What the body of a card says is not its title.
    card_names(user, TEST_WS, &other, &[&mine.card]).await;

    let told = found(user, TEST_WS, "quarterly").await;
    told.check_ok();
    assert_eq!(
        told.value(),
        json!([
            {"id": mine.page, "kind": "page", "title": "Quarterly Plan", "icon": "",
                "containerId": mine.wiki, "containerKind": "wiki"},
            {"id": mine.card, "kind": "card", "title": "Quarterly Plan", "number": 1,
                "icon": "", "containerId": mine.board, "containerKind": "board"},
        ]),
    );
    for (term, expected) in [
        ("PLAN%20quarter", vec![&mine.page, &mine.card]),
        ("plan", vec![&accented, &mine.page, &mine.card]),
        ("%C3%A9tude", vec![&accented]),
        ("%25", vec![&percent]),
        ("_", vec![&percent]),
        // `_` is no letter standing for any other: "d_ne" is not "done".
        ("d_ne", vec![]),
        ("see", vec![]),
        ("plan%20nothing", vec![]),
        (
            "",
            vec![&other, &accented, &percent, &mine.page, &mine.card],
        ),
        (
            "%20%20",
            vec![&other, &accented, &percent, &mine.page, &mine.card],
        ),
    ] {
        let expected: Vec<&str> = expected.into_iter().map(String::as_str).collect();
        assert_eq!(
            listed(&found(user, TEST_WS, term).await),
            expected,
            "{term:?}"
        );
    }
    let without_q = user.get(&format!("{TEST_WS}/references/search")).await;
    assert_eq!(listed(&without_q).len(), 5);
    // A term is at most 200 characters, of whatever letters.
    found(user, TEST_WS, &"%C3%A9".repeat(200)).await.check_ok();
    let long = found(user, TEST_WS, &"%C3%A9".repeat(201)).await;
    assert_eq!(long.status, StatusCode::BAD_REQUEST, "{}", long.text());

    // In the trash a card is not found; a card that is a template of the board's
    // cards never is, nor what is on a board that is a template.
    user.delete(&block_path(&mine.board, &percent))
        .await
        .check_no_content();
    assert_eq!(found(user, TEST_WS, "done").await.text(), "[]");
    user.post(
        &format!("{TEST_WS}/boards/{}/cards", mine.board),
        &json!({"title": "plan of a template", "isTemplate": true}),
    )
    .await
    .check_created();
    let template: Container = user
        .create_board(&Container {
            is_template: true,
            ..crate::boards::new_board(
                crate::common::TEST_WORKSPACE_ID,
                fvoci_model::ContainerVisibility::Open,
                "Template",
            )
        })
        .await
        .created_json();
    let on_template = card_of(user, TEST_WS, &template.id, "plan on a template").await;
    assert_eq!(
        listed(&found(user, TEST_WS, "plan").await),
        [accented.as_str(), mine.page.as_str(), mine.card.as_str()]
    );
    // What is on the template names as anything does, to who reads it.
    card_names(user, TEST_WS, &on_template, &[&mine.card]).await;
    assert!(listed(&naming(user, TEST_WS, &mine.card, "").await).contains(&on_template));

    // Twenty at most, the ones changed last.
    let mut many = Vec::new();
    for number in 0..22 {
        many.push(card_of(user, TEST_WS, &mine.board, &format!("many {number}")).await);
    }
    many.reverse();
    assert_eq!(listed(&found(user, TEST_WS, "many").await), many[..20]);
    assert_eq!(listed(&found(user, TEST_WS, "").await), many[..20]);
    th.teardown().await;
}

/// Nothing is found of what the asker does not read: of a board or a wiki they are
/// not on, of an open board they have not joined, of another workspace; nor without a
/// session. An open wiki they read unjoined, and what is on it is found.
#[tokio::test]
async fn what_the_asker_does_not_read_is_not_found() {
    let th = Setup::native().start().await.init_basic().await;
    let (owner, member) = (&th.client, &th.client2);
    let own = made(member, TEST_WS, "private", "own plan").await;
    let before = found(member, TEST_WS, "").await.text();
    let private = made(owner, TEST_WS, "private", HIDDEN).await;
    let open = made(owner, TEST_WS, "open", HIDDEN).await;
    made(owner, ROOT_WS, "open", HIDDEN).await;

    assert_eq!(listed(&found(owner, TEST_WS, HIDDEN).await).len(), 4);
    for term in [HIDDEN, "hidden", "title", "HIDDEN-TITLE"] {
        let of_term = listed(&found(member, TEST_WS, term).await);
        assert_eq!(of_term, [open.page.as_str(), open.card.as_str()], "{term}");
    }
    let recent = found(member, TEST_WS, "").await;
    assert_ne!(recent.text(), before, "the open board and wiki are new");
    assert_eq!(
        listed(&recent),
        [
            open.page.as_str(),
            open.card.as_str(),
            own.page.as_str(),
            own.card.as_str()
        ]
    );

    // Invited to private.wiki: what is there is found from then on.
    let_read(owner, &private.wiki, &member.me().await.id).await;
    assert_eq!(
        listed(&found(member, TEST_WS, HIDDEN).await),
        [
            open.page.as_str(),
            open.card.as_str(),
            private.page.as_str()
        ]
    );
    let refused = th
        .anonymous()
        .get(&format!("{TEST_WS}/references/search?q={HIDDEN}"))
        .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert!(!refused.text().contains(HIDDEN));
    th.teardown().await;
}
