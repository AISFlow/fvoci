//! The pages of wikis in the search of bodies: sent with the cards, found by who reads
//! their wiki, and shown to nobody else, whatever the index says.

use std::collections::BTreeMap;

use fvoci_model::{
    Container, ContainerKind, ContainerVisibility, NewPage, Page, SearchHit, SearchKind, new_id,
};
use fvoci_server::search::tend;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Store};
use serde_json::{Value, json};

use crate::boards::new_board;
use crate::common::{
    Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, ROOT_WS, TEST_WORKSPACE_ID, TEST_WS, TestServer,
};
use crate::content_search::{INDEX, new_card, started};
use crate::fake_search::{FakeSearch, page_document};
use crate::workspaces::another_organization;

fn body(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})
}

/// A private wiki of the test workspace made by `client`'s user.
pub(crate) async fn new_wiki(client: &Client) -> String {
    let made = client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("an id").to_owned()
}

/// A page of a wiki made through the API, below `parent_id` (empty for the top), with
/// a body of one paragraph.
pub(crate) async fn new_page(
    client: &Client,
    wiki_id: &str,
    parent_id: &str,
    title: &str,
    text: &str,
) -> Page {
    let pages = format!("{TEST_WS}/wikis/{wiki_id}/pages");
    let made: Page = client
        .post(&pages, &json!({"title": title, "parentId": parent_id}))
        .await
        .created_json();
    client
        .patch(
            &format!("{pages}/{}", made.id),
            &json!({"body": body(text), "readAt": made.updated_at}),
        )
        .await
        .ok_json()
}

/// A wiki stored past the API, with a page of it: in any workspace of the store's
/// organization, by any user, for the invited only or open to who works there. The
/// identifiers of both.
pub(crate) async fn stored_wiki(
    store: &Store,
    workspace_id: &str,
    wiki_type: ContainerVisibility,
    user_id: &str,
) -> (String, String) {
    let mut wiki = Container {
        id: new_id(),
        kind: ContainerKind::Wiki,
        ..new_board(workspace_id, wiki_type, "Wiki")
    };
    store
        .insert_container_with_admin(&mut wiki, user_id)
        .await
        .expect("the wiki is stored");
    let page = store
        .create_page(
            &wiki.id,
            &NewPage {
                title: "환불 규정".to_owned(),
                ..NewPage::default()
            },
            fvoci_model::PageLimits::default(),
            user_id,
        )
        .await
        .expect("the page is stored")
        .first;
    (wiki.id, page.id)
}

/// What the search server holds: the kind and the container of each document, by the
/// identifier of its card or page.
fn held(fake: &FakeSearch) -> BTreeMap<String, (String, String)> {
    fake.documents(INDEX)
        .into_iter()
        .map(|(id, document)| {
            let text = |member: &str| document[member].as_str().unwrap_or_default().to_owned();
            (id, (text("kind"), text("containerId")))
        })
        .collect()
}

async fn hits(client: &Client, workspace: &str, parameters: &str) -> Vec<SearchHit> {
    client
        .get(&format!(
            "{workspace}/search?q=%ED%99%98%EB%B6%88{parameters}"
        ))
        .await
        .ok_json()
}

fn ids(hits: &[SearchHit]) -> Vec<String> {
    let mut ids: Vec<String> = hits.iter().map(|hit| hit.id.clone()).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_page_is_sent_and_found_with_the_cards_while_it_is_there() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let wiki = new_wiki(&th.client).await;
    let page = new_page(&th.client, &wiki, "", "환불 안내", "환불규정을 따른다").await;
    let below = new_page(&th.client, &wiki, &page.id, "환불 예외", "환불").await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Support",
        ))
        .await
        .created_json();
    let card = new_card(&th.client, &board.id, "환불 카드", "환불").await;

    tend(&th.app).await;
    let sent = held(&fake);
    let all = hits(&th.client, TEST_WS, "").await;
    let pages = hits(&th.client, TEST_WS, "&kind=page").await;
    let cards = hits(&th.client, TEST_WS, "&kind=card").await;
    // The page goes to the trash, and what is below it with it.
    th.client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{}", page.id))
        .await
        .check_no_content();
    tend(&th.app).await;
    let after_the_page = held(&fake);
    // A page of its own, which gets another title; then the wiki goes to the trash
    // and comes back.
    let kept = new_page(&th.client, &wiki, "", "환불 문의", "환불").await;
    tend(&th.app).await;
    th.client
        .patch(
            &format!("{TEST_WS}/wikis/{wiki}/pages/{}", kept.id),
            &json!({"title": "환불 문의처"}),
        )
        .await
        .check_ok();
    tend(&th.app).await;
    let retitled = fake.documents(INDEX);
    th.client
        .delete(&format!("{TEST_WS}/containers/{wiki}"))
        .await
        .check_no_content();
    tend(&th.app).await;
    let after_the_wiki = held(&fake);
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{wiki}/restore"))
        .await
        .check_ok();
    tend(&th.app).await;
    let back = hits(&th.client, TEST_WS, "&kind=page").await;
    th.teardown().await;

    let as_page = ("page".to_owned(), wiki.clone());
    assert_eq!(sent[&page.id], as_page);
    assert_eq!(sent[&below.id], as_page);
    assert_eq!(sent[&card.id], ("card".to_owned(), board.id.clone()));
    let mut both = vec![page.id.clone(), below.id.clone()];
    both.sort();
    let mut every = vec![page.id.clone(), below.id.clone(), card.id.clone()];
    every.sort();
    assert_eq!(ids(&all), every, "pages and cards alike");
    assert_eq!(ids(&pages), both);
    assert_eq!(ids(&cards), std::slice::from_ref(&card.id));
    let found = pages.iter().find(|hit| hit.id == page.id).expect("found");
    assert_eq!(found.kind, SearchKind::Page);
    assert_eq!(found.container_id, wiki);
    assert_eq!(found.container_kind, ContainerKind::Wiki);
    assert_eq!(found.title, "환불 안내");
    assert!(found.snippet.iter().any(|part| part.matched), "{found:?}");
    let of_card = cards.first().expect("the card");
    assert_eq!(
        (of_card.kind, of_card.container_kind),
        (SearchKind::Card, ContainerKind::Board)
    );
    assert_eq!(
        after_the_page.into_keys().collect::<Vec<_>>(),
        std::slice::from_ref(&card.id),
        "the page went, and the page below it"
    );
    assert_eq!(
        retitled[&kept.id]["title"], "환불 문의처",
        "sent with the title it has now, before its wiki went"
    );
    assert_eq!(
        after_the_wiki.into_keys().collect::<Vec<_>>(),
        [card.id],
        "a wiki in the trash"
    );
    assert_eq!(
        ids(&back),
        [kept.id],
        "the wiki is back with the page it had"
    );
}

/// A page of a wiki of another organization on the server of the test.
async fn page_of_another_organization(th: &TestServer) -> String {
    let (theirs, their_workspace, _) = another_organization(th).await;
    let mut their_user = fvoci_model::User {
        id: new_id(),
        username: "writer.elsewhere".to_owned(),
        email: "writer.elsewhere@sample.com".to_owned(),
        ..fvoci_model::User::default()
    };
    theirs
        .create_user(&mut their_user)
        .await
        .expect("their user is stored");
    let (_, page) = stored_wiki(
        &theirs,
        &their_workspace,
        ContainerVisibility::Private,
        &their_user.id,
    )
    .await;
    page
}

/// The search server answers every search with pages the searcher does not read: of a
/// private wiki of somebody else, of another workspace and another organization, of
/// the trash (a page and what was below it) and of a wiki that is gone for good.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every kind of page the searcher does not read, in one stale index"
)]
async fn a_stale_index_shows_nobody_a_page_they_do_not_read() {
    let fake = FakeSearch::start().await;
    let th = started(&fake).await;
    let org_id = th.app.store().tenant().org_id().expect("an organization");
    let store = th.app.store();
    // A board of the first user in the root workspace: a search there asks the index.
    th.create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    // The first user's wiki: a page they read, and a page that went with one below.
    let mine = new_wiki(&th.client).await;
    let read = new_page(&th.client, &mine, "", "읽는 페이지", "환불").await;
    let trashed = new_page(&th.client, &mine, "", "지운 페이지", "환불").await;
    let below = new_page(&th.client, &mine, &trashed.id, "그 아래 페이지", "환불").await;
    th.client
        .delete(&format!("{TEST_WS}/wikis/{mine}/pages/{}", trashed.id))
        .await
        .check_no_content();
    // A wiki of the first user that went for good, with its page.
    let gone = new_wiki(&th.client).await;
    let gone_page = new_page(&th.client, &gone, "", "사라진 위키의 페이지", "환불").await;
    th.client
        .delete(&format!("{TEST_WS}/containers/{gone}"))
        .await
        .check_no_content();
    th.app
        .housekeeping()
        .store()
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is purged");
    // A private wiki of somebody else in the same workspace, and one of a workspace
    // the searchers are not in.
    let stranger = th.stranger_in(TEST_WORKSPACE_ID).await;
    let (private, private_page) = stored_wiki(
        store,
        TEST_WORKSPACE_ID,
        ContainerVisibility::Private,
        &stranger,
    )
    .await;
    // An open wiki the searchers have not joined: read by whoever works there.
    let (open, open_page) = stored_wiki(
        store,
        TEST_WORKSPACE_ID,
        ContainerVisibility::Open,
        &stranger,
    )
    .await;
    let hidden_owner = th.stranger_in(HIDDEN_WORKSPACE_ID).await;
    let (_, hidden_page) = stored_wiki(
        store,
        HIDDEN_WORKSPACE_ID,
        ContainerVisibility::Private,
        &hidden_owner,
    )
    .await;

    let mut planted = vec![
        (read.id.clone(), mine.clone()),
        (trashed.id.clone(), mine.clone()),
        (below.id.clone(), mine.clone()),
        (gone_page.id.clone(), gone.clone()),
        (private_page, private),
        (open_page.clone(), open.clone()),
        (hidden_page, mine.clone()),
    ];
    // Another organization's page, where the database holds more than one.
    if db_type_of_the_tests() == DbType::Postgres {
        let their_page = page_of_another_organization(&th).await;
        planted.push((their_page, mine.clone()));
    }
    for (id, container) in &planted {
        fake.plant(
            id,
            page_document(&org_id, TEST_WORKSPACE_ID, container, "환불 규정", "환불"),
        );
    }
    // The second user has a board of their own to search, with a card, and no wiki.
    let their_board: Container = th
        .client2
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Theirs",
        ))
        .await
        .created_json();
    let their_card = new_card(&th.client2, &their_board.id, "환불 카드", "환불").await;
    fake.plant(
        &their_card.id,
        crate::fake_search::document(&org_id, TEST_WORKSPACE_ID, &their_board.id, "x", "x"),
    );

    let owner = hits(&th.client, TEST_WS, "").await;
    let other = hits(&th.client2, TEST_WS, "").await;
    let in_root = hits(&th.client, ROOT_WS, "").await;
    let asked = fake.searches().len();
    th.teardown().await;

    let mut read_by_owner = vec![read.id.clone(), open_page.clone()];
    read_by_owner.sort();
    assert_eq!(
        ids(&owner),
        read_by_owner,
        "the page of their wiki and the page of the open wiki, and no other"
    );
    let own = owner
        .iter()
        .find(|hit| hit.id == read.id)
        .expect("their page");
    assert_eq!(own.container_id, mine);
    assert_eq!(own.title, "읽는 페이지", "the title is the database's");
    assert!(
        own.snippet.is_empty(),
        "what was indexed from another title and text is not shown: {owner:?}"
    );
    let of_open = owner.iter().find(|hit| hit.id == open_page).expect("open");
    assert_eq!(of_open.container_id, open);
    let mut read_by_other = vec![their_card.id, open_page];
    read_by_other.sort();
    assert_eq!(
        ids(&other),
        read_by_other,
        "their own card, and the page of the open wiki: no other wiki is theirs"
    );
    assert_eq!(ids(&in_root), Vec::<String>::new(), "another workspace");
    assert_eq!(asked, 3, "each of them asked the index");
}
