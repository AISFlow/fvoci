//! No way a body leaves the server tells the title of what the body names.
//!
//! A reference is an identifier, and the title of what it names is told by
//! `resolveReferences` alone, to who reads that. Here a reader is given everything the
//! server gives of a card that names things the reader does not read: the card, the
//! blocks and the cards of its board with their previews, the versions of its body,
//! the archive of the board, the room of the body, what a connection is told of a
//! change, and what a webhook is posted. Each is searched for the title. So is what
//! the server gives of a page that names such things.

use std::io::{Cursor, Read as _};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use fvoci_model::{Block, BlockType};
use serde_json::{Value, json};

use crate::common::{Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::references::{HIDDEN, made};
use crate::references_shared::references_in;
use crate::references_to::{body_naming, card_names, card_of, comment_naming, page_names};
use crate::rooms::{Peer, give};
use crate::websocket::{Socket, subscribe_to_workspace};

/// One thing the server gave a reader.
struct Given {
    what: String,
    succeeded: bool,
    bytes: Vec<u8>,
}

impl Given {
    fn tells(&self, text: &str) -> bool {
        let text = text.as_bytes();
        self.bytes.windows(text.len()).any(|part| part == text)
    }
}

/// A board of the test workspace the first user made, which the second user edits,
/// with a card that names `targets` in its body and in a comment on it: the board and
/// the card.
async fn source_naming(th: &TestServer, targets: &[&str]) -> (String, String) {
    let owner = &th.client;
    let held = made(owner, TEST_WS, "private", "held").await;
    give(th, &held.board, &th.client2.me().await.id, "editor").await;
    card_names(owner, TEST_WS, &held.card, targets).await;
    comment_naming(owner, &held.board, &held.card, targets).await;
    (held.board, held.card)
}

/// Everything the server gives `reader` of the card `source` of the board.
async fn given(reader: &Client, board: &str, source: &str) -> Vec<Given> {
    let mut all = Vec::new();
    let mut paths = vec![
        format!("{TEST_WS}/cards/{source}"),
        format!("{TEST_WS}/boards/{board}/blocks?all=true"),
        format!("{TEST_WS}/boards/{board}/blocks?parentId={source}&type=comment"),
        format!("{TEST_WS}/boards/{board}/cards?preview=true"),
        format!("{TEST_WS}/references/to/{source}"),
        format!("{TEST_WS}/references/search?q={HIDDEN}"),
        format!("{TEST_WS}/references/search?q="),
    ];
    paths.extend(versions_of(reader, source).await);
    for path in paths {
        all.push(got(reader, path).await);
    }
    let archive = reader
        .get(&format!("{TEST_WS}/containers/{board}/archive/export"))
        .await;
    let mut entries = Vec::new();
    let archive_ok = if archive.status == reqwest::StatusCode::FORBIDDEN {
        // Workspace guests cannot export container archives (403); verify the refusal
        // body still carries no hidden title.
        entries.extend_from_slice(&archive.body);
        true
    } else {
        if let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(archive.body.clone())) {
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index).expect("an entry");
                let name = entry.name().expect("a name").into_owned();
                entries.extend(name.into_bytes());
                entry.read_to_end(&mut entries).expect("the entry is read");
            }
        }
        archive.status.is_success() && !entries.is_empty()
    };
    all.push(Given {
        what: "the archive".to_owned(),
        succeeded: archive_ok,
        bytes: entries,
    });

    all.push(room_of(reader, source).await);
    all
}

/// What `reader` is answered at the path.
async fn got(reader: &Client, path: String) -> Given {
    let answer = reader.get(&path).await;
    Given {
        succeeded: answer.status.is_success(),
        bytes: answer.body,
        what: path,
    }
}

/// The paths of the versions of the body of a card, and of their list.
async fn versions_of(reader: &Client, id: &str) -> Vec<String> {
    let versions = format!("{TEST_WS}/cards/{id}/body-versions");
    let listed = reader.get(&versions).await;
    let mut paths: Vec<String> = listed
        .value()
        .as_array()
        .into_iter()
        .flatten()
        .map(|version| format!("{versions}/{}", version["version"]))
        .collect();
    paths.push(versions);
    paths
}

/// The room of the body of a card or a page: what it answers, and every frame of the
/// document.
async fn room_of(reader: &Client, id: &str) -> Given {
    let mut peer = Peer::connect(reader, 77).await;
    let answer = peer.join(TEST_WORKSPACE_ID, id).await;
    let mut frames = answer.to_string().into_bytes();
    for frame in &peer.taken {
        frames.extend(&frame.payload);
    }
    Given {
        what: "the room".to_owned(),
        succeeded: answer["type"] == "docJoined" && !peer.taken.is_empty(),
        bytes: frames,
    }
}

/// What a connection of `reader` that follows the workspace is told when the owner
/// writes the card or the page at `path` again, naming `targets`.
async fn told_of_a_change(th: &TestServer, reader: &Client, path: &str, targets: &[&str]) -> Given {
    let mut socket = Socket::connect_as(reader).await;
    subscribe_to_workspace(th, &mut socket, TEST_WORKSPACE_ID, &reader.me().await.id).await;
    let source = path.rsplit('/').next().expect("a path");
    let read_at = th.client.get(path).await.value()["updatedAt"].clone();
    th.client
        .patch(
            path,
            &json!({"readAt": read_at, "body": body_naming("again", targets)}),
        )
        .await
        .check_ok();
    let mut told = Vec::new();
    let mut of_the_card = false;
    while let Some(message) = socket.next_json(Duration::from_millis(500)).await {
        of_the_card |= message["type"] == "blockChanged" && message["block"]["id"] == source;
        told.extend(message.to_string().into_bytes());
    }
    Given {
        what: "the change".to_owned(),
        succeeded: of_the_card,
        bytes: told,
    }
}

/// Gives the second user everything of a card that names `targets`, none of which that
/// user reads, and looks for the title in it.
async fn check_nothing_tells_the_title(th: &TestServer, targets: &[&str]) {
    let reader = &th.client2;
    let (board, source) = source_naming(th, targets).await;
    let mut all = given(reader, &board, &source).await;
    let path = format!("{TEST_WS}/cards/{source}");
    all.push(told_of_a_change(th, reader, &path, targets).await);
    check_all(&all);
    // The identifiers are the body's, and go where the body goes.
    let card = reader.get(&path).await;
    card.check_ok();
    assert_eq!(
        references_in(&card.value()["body"]),
        targets,
        "the body names by identifier"
    );
}

/// Checks that nothing given tells the title, and that every way gave something: a
/// refusal tells no title, and would pin nothing.
fn check_all(all: &[Given]) {
    for one in all {
        assert!(!one.tells(HIDDEN), "{} tells the title", one.what);
        assert!(
            one.succeeded,
            "{} gave nothing: {}",
            one.what,
            String::from_utf8_lossy(&one.bytes)
        );
    }
}

/// A page that names what its reader does not read: the page, the pages of its wiki,
/// what names it, the room of its body, and what a connection is told when it changes.
/// The versions of a page's body have no route yet.
#[tokio::test]
async fn nothing_a_page_gives_tells_the_title_of_what_it_names() {
    let th = Setup::native().start().await.init_basic().await;
    let (owner, reader) = (&th.client, &th.client2);
    let private = made(owner, TEST_WS, "private", HIDDEN).await;
    let held = made(owner, TEST_WS, "private", "held").await;
    give(&th, &held.wiki, &reader.me().await.id, "editor").await;
    let targets = private.both();
    page_names(owner, TEST_WS, &held, &targets).await;

    let pages = format!("{TEST_WS}/wikis/{}/pages", held.wiki);
    let page = format!("{pages}/{}", held.page);
    let to_it = format!("{TEST_WS}/references/to/{}", held.page);
    let mut all = Vec::new();
    for path in [page.clone(), pages, to_it] {
        all.push(got(reader, path).await);
    }
    all.push(room_of(reader, &held.page).await);
    all.push(told_of_a_change(&th, reader, &page, &targets).await);
    check_all(&all);
    let read = reader.get(&page).await;
    assert_eq!(references_in(&read.value()["body"]), targets);
    th.teardown().await;
}

/// A member of the workspace who is on neither the private board nor the private wiki
/// of what is named (open boards and open wikis are read unjoined by workspace members).
#[tokio::test]
async fn nothing_tells_a_member_the_title_of_what_is_on_a_board_they_are_not_on() {
    let th = Setup::native().start().await.init_basic().await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let open = made(&th.client, TEST_WS, "open", "read unjoined").await;
    let targets = [private.card.as_str(), &private.page, &open.card, &open.page];

    check_nothing_tells_the_title(&th, &targets).await;
    th.teardown().await;
}

/// A guest of the workspace, invited to the board of the card and to nothing else.
#[tokio::test]
async fn nothing_tells_a_guest_the_title_of_what_they_were_not_invited_to() {
    let th = Setup::native().start().await.init_basic().await;
    crate::guests::guest_of_the_workspace(&th).await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let open = made(&th.client, TEST_WS, "open", HIDDEN).await;
    let targets = [private.both(), open.both()].concat();
    check_nothing_tells_the_title(&th, &targets).await;
    th.teardown().await;
}

/// An administrator of the workspace, who manages the board of what is named and reads
/// nothing on it.
#[tokio::test]
async fn nothing_tells_who_only_manages_the_title_of_what_is_on_the_board() {
    let th = Setup::native().start().await.init_basic().await;
    crate::workspace_admin::workspace_admin(&th).await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    check_nothing_tells_the_title(&th, &private.both()).await;
    th.teardown().await;
}

/// What is named is of another workspace, and of another organization.
#[tokio::test]
async fn nothing_tells_the_title_of_what_is_of_another_workspace_or_organization() {
    let th = Setup::native().start().await.init_basic().await;
    let root = made(&th.client, ROOT_WS, "private", HIDDEN).await;
    let mut targets: Vec<String> = root.both().map(str::to_owned).to_vec();
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
        let (theirs, _, their_board) = crate::workspaces::another_organization(&th).await;
        let members = theirs.get_members_for_container(&their_board).await;
        let owner = members.expect("their members")[0].user_id.clone();
        let mut card = Block {
            id: fvoci_model::new_id(),
            container_id: their_board,
            block_type: BlockType::Card,
            title: HIDDEN.to_owned(),
            ..Block::default()
        };
        theirs
            .insert_block(&mut card, &owner)
            .await
            .expect("their card is stored");
        targets.push(card.id);
    }
    let targets: Vec<&str> = targets.iter().map(String::as_str).collect();
    check_nothing_tells_the_title(&th, &targets).await;
    th.teardown().await;
}

/// Where the webhook's receiver listens. The server's configuration is changed by a
/// plain function, which finds the address here.
static WEBHOOK_URL: OnceLock<String> = OnceLock::new();

/// A webhook is posted the card that names, with the identifiers of its body and
/// without the title of anything it names.
#[tokio::test]
async fn a_webhook_is_posted_what_a_body_names_by_identifier_alone() {
    let posted: Arc<Mutex<Vec<Value>>> = Arc::default();
    let keep = {
        let posted = Arc::clone(&posted);
        move |axum::Json(block): axum::Json<Value>| async move {
            posted.lock().expect("lock poisoned").push(block);
        }
    };
    let hook = axum::Router::new().route("/hook", axum::routing::post(keep));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a local port is free");
    let address = listener.local_addr().expect("the listener has an address");
    WEBHOOK_URL
        .set(format!("http://{address}/hook"))
        .expect("only one test sets the webhook address");
    let serving = tokio::spawn(async move {
        axum::serve(listener, hook)
            .await
            .expect("the webhook receiver serves");
    });
    let th = Setup::native()
        .configure(|config| {
            config.block_change_webhooks = WEBHOOK_URL.get().into_iter().cloned().collect();
        })
        .start()
        .await
        .init_basic()
        .await;
    let private = made(&th.client, TEST_WS, "private", HIDDEN).await;
    let held = made(&th.client, TEST_WS, "private", "held").await;
    let source = card_of(&th.client, TEST_WS, &held.board, "source").await;
    card_names(&th.client, TEST_WS, &source, &private.both()).await;
    comment_naming(&th.client, &held.board, &source, &private.both()).await;
    assert!(
        th.app.finish_background(Duration::from_secs(10)).await,
        "the webhook calls finish"
    );

    let posted: Vec<Value> = posted.lock().expect("lock poisoned").clone();
    let of_the_source: Vec<&Value> = posted
        .iter()
        .filter(|block| block["id"] == source.as_str() || block["parentId"] == source.as_str())
        .collect();
    let naming: Vec<&&Value> = of_the_source
        .iter()
        .filter(|block| references_in(&block["body"]) == private.both())
        .collect();
    assert_eq!(
        naming.len(),
        2,
        "the card and its comment: {of_the_source:?}"
    );
    for block in of_the_source {
        assert!(!block.to_string().contains(HIDDEN), "{block}");
    }
    serving.abort();
    th.teardown().await;
}

/// A version of a body is a body: it names by identifier what the body named then.
/// What names a block is counted from the body a card has now, whatever its versions
/// named, and a version written back names again.
#[tokio::test]
async fn a_version_keeps_what_the_body_named_and_only_the_body_of_now_names() {
    let th = Setup::native().start().await.init_basic().await;
    let user = &th.client;
    let mine = made(user, TEST_WS, "private", "mine").await;
    let before = card_of(user, TEST_WS, &mine.board, "named before").await;
    let now = card_of(user, TEST_WS, &mine.board, "named now").await;
    let source = mine.card.as_str();
    let naming = async |target: &str| -> Vec<String> {
        let answer = user
            .get(&format!("{TEST_WS}/references/to/{target}"))
            .await
            .value();
        let listed = answer.as_array().expect("a list").iter();
        listed
            .map(|linked| linked["id"].as_str().expect("an id").to_owned())
            .collect()
    };
    card_names(user, TEST_WS, source, &[&before]).await;
    card_names(user, TEST_WS, source, &[&now]).await;
    assert_eq!(naming(&before).await, [""; 0]);
    assert_eq!(naming(&now).await, [source]);

    let versions = format!("{TEST_WS}/cards/{source}/body-versions");
    let listed = user.get(&versions).await.value();
    let mut of_before = None;
    for version in listed.as_array().expect("the versions") {
        let read = user
            .get(&format!("{versions}/{}", version["version"]))
            .await;
        read.check_ok();
        assert!(!read.text().contains("named before"), "{}", read.text());
        if references_in(&read.value()["body"]) == [before.as_str()] {
            of_before = version["version"].as_u64();
        }
    }
    let of_before = of_before.expect("a version names what the body named then");
    user.post_empty(&format!("{versions}/{of_before}/restore"))
        .await
        .check_ok();
    assert_eq!(naming(&before).await, [source]);
    assert_eq!(naming(&now).await, [""; 0]);
    th.teardown().await;
}
