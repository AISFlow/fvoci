//! Departed authors remain named only through work the reader can read.

use fvoci_model::{
    Block, BlockPatch, BlockType, Body, BodyChange, Container, ContainerMember,
    ContainerVisibility, NewPage, User, new_id,
};
use serde_json::json;

use crate::boards::{member, new_board};
use crate::common::{
    Client, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer,
};

#[derive(Clone, Copy)]
enum Authorship {
    Board,
    CommentVersion,
    TrashedCommentVersion,
    BodyVersion,
    PageVersion,
    FileVersion,
}

fn body(text: &str) -> Body {
    Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]}))
    .expect("valid body")
    .expect("nonempty body")
}

/// A wiki of the owner with a page whose body the leaver wrote a version of.
async fn authored_wiki(th: &TestServer, leaver: &str) -> Container {
    let owner = th.client.me().await.id;
    let wiki: Container = th
        .client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await
        .created_json();
    versioned_page(th, &wiki.id, &owner, leaver).await;
    wiki
}

async fn authored_board(th: &TestServer, authorship: Authorship, leaver: &str) -> Container {
    let owner = th.client.me().await.id;
    let creator = match authorship {
        Authorship::Board => &th.client2,
        Authorship::CommentVersion
        | Authorship::TrashedCommentVersion
        | Authorship::BodyVersion
        | Authorship::PageVersion
        | Authorship::FileVersion => &th.client,
    };
    let board: Container = creator
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Authored",
        ))
        .await
        .created_json();
    if matches!(authorship, Authorship::Board) {
        let as_admin = member(&board.id, &owner, fvoci_model::ContainerRole::Admin);
        creator.add_member(&as_admin).await.check_ok();
    } else if matches!(authorship, Authorship::BodyVersion) {
        versioned_card(th, &board.id, &owner, leaver).await;
    } else if matches!(authorship, Authorship::FileVersion) {
        versioned_file(th, &board.id, &owner, leaver).await;
    } else {
        let mut card = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            ..Block::default()
        };
        th.app
            .store()
            .insert_block(&mut card, &owner)
            .await
            .expect("a card");
        let mut comment = Block {
            id: new_id(),
            container_id: board.id.clone(),
            parent_id: card.id,
            block_type: BlockType::Comment,
            body: Some(body("Original")),
            ..Block::default()
        };
        th.app
            .store()
            .insert_block(&mut comment, &owner)
            .await
            .expect("a comment");
        for (writer, text) in [(leaver, "An earlier revision"), (owner.as_str(), "Current")] {
            th.app
                .store()
                .patch_block(
                    &comment.id,
                    &BlockPatch {
                        body: BodyChange::Set(body(text)),
                        ..BlockPatch::default()
                    },
                    writer,
                )
                .await
                .expect("a revised comment");
        }
        let current = th
            .app
            .store()
            .get_block(&comment.id)
            .await
            .expect("the comment");
        assert_eq!(
            current.created_by, owner,
            "the leaver is not the original writer"
        );
        assert_eq!(
            current.modified_by, owner,
            "the leaver is not the last writer"
        );
        if matches!(authorship, Authorship::TrashedCommentVersion) {
            th.app
                .store()
                .delete_block(&comment.id, &owner)
                .await
                .expect("the comment is trashed");
            th.client
                .get(&format!(
                    "{TEST_WS}/boards/{}/cards/{}/comments/{}/versions",
                    board.id, comment.parent_id, comment.id
                ))
                .await
                .check_ok();
        }
    }
    board
}

/// A card of the board whose body the owner made and wrote last, with an earlier
/// version of it that the leaver wrote.
async fn versioned_card(th: &TestServer, board_id: &str, owner: &str, leaver: &str) {
    let store = th.app.store();
    let mut card = Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        body: Some(body("Original")),
        ..Block::default()
    };
    store.insert_block(&mut card, owner).await.expect("a card");
    for (writer, text) in [(leaver, "An earlier version"), (owner, "Current")] {
        let patch = BlockPatch {
            body: BodyChange::Set(body(text)),
            ..BlockPatch::default()
        };
        store
            .patch_block(&card.id, &patch, writer)
            .await
            .expect("a body written");
    }
    let block = store.get_block(&card.id).await.expect("the card");
    assert_eq!(
        (block.created_by.as_str(), block.modified_by.as_str()),
        (owner, owner),
        "the leaver neither made the card nor changed it last"
    );
    let versions = store.body_versions(&card.id).await.expect("the versions");
    assert_eq!(versions[1].editors, [leaver], "the version before the last");
}

/// A file of the board that the owner uploaded and wrote the newest version of, with a
/// version between the two that the leaver wrote.
async fn versioned_file(th: &TestServer, board_id: &str, owner: &str, leaver: &str) {
    let upload = async |by: &str| {
        th.app
            .save_file_by(&mut &b"some bytes"[..], board_id, "paper.txt", by)
            .await
            .expect("a file is stored")
    };
    let file = upload(owner).await;
    for (base, writer) in [(1, leaver), (2, owner)] {
        let next = upload(writer).await;
        let added = th
            .app
            .store()
            .add_file_version(board_id, &file, base, &next, writer)
            .await
            .expect("a version is written");
        assert!(
            matches!(added, fvoci_store::VersionAdded::Written(_)),
            "{added:?}"
        );
    }
    let versions = th
        .app
        .store()
        .file_versions(board_id, &file)
        .await
        .expect("the versions");
    let writers: Vec<&str> = versions
        .iter()
        .map(|version| version.created_by.as_str())
        .collect();
    assert_eq!(writers, [owner, leaver, owner], "the newest first");
}

/// A page of the wiki that the owner made and wrote last, with an earlier version of
/// its body that the leaver wrote.
async fn versioned_page(th: &TestServer, wiki_id: &str, owner: &str, leaver: &str) {
    let store = th.app.store();
    let page = store
        .create_page(
            wiki_id,
            &NewPage::default(),
            fvoci_model::PageLimits::default(),
            owner,
        )
        .await
        .expect("a page")
        .first;
    for (writer, text) in [(leaver, "An earlier version"), (owner, "Current")] {
        let patch = BlockPatch {
            body: BodyChange::Set(body(text)),
            ..BlockPatch::default()
        };
        store
            .patch_block(&page.id, &patch, writer)
            .await
            .expect("a body written");
    }
    let block = store.get_block(&page.id).await.expect("the page");
    assert_eq!(
        (block.created_by.as_str(), block.modified_by.as_str()),
        (owner, owner),
        "the leaver neither made the page nor changed it last"
    );
    let versions = store.body_versions(&page.id).await.expect("the versions");
    assert_eq!(versions[1].editors, [leaver], "the version before the last");
}

async fn named(client: &Client, workspace: &str, user: &str) -> Vec<User> {
    client
        .post(&format!("{workspace}/users"), &[user])
        .await
        .ok_json()
}

async fn check_authorship(authorship: Authorship) {
    let th = Setup::native().start().await.init_basic().await;
    let leaver = th.client2.me().await.id;
    let board = match authorship {
        Authorship::PageVersion => authored_wiki(&th, &leaver).await,
        _ => authored_board(&th, authorship, &leaver).await,
    };
    for workspace in [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID] {
        th.app.store().execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{leaver}' AND workspace_id = '{workspace}'"
        )).await.expect("the author leaves");
    }
    let by_id = named(&th.client, TEST_WS, &leaver).await;
    assert_eq!(
        by_id
            .iter()
            .map(|user| user.id.as_str())
            .collect::<Vec<_>>(),
        [leaver.as_str()],
        "a readable contribution still names its departed author"
    );
    th.client
        .get(&format!("{TEST_WS}/users/{leaver}"))
        .await
        .check_ok();
    assert!(
        named(&th.client, ROOT_WS, &leaver).await.is_empty(),
        "no contribution in another workspace"
    );
    let found: Vec<User> = th
        .client
        .get(&format!("{TEST_WS}/users?search=user2"))
        .await
        .ok_json();
    assert!(
        found.is_empty(),
        "an author is not offered among current people"
    );
    let members: Vec<ContainerMember> = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/members", board.id))
        .await
        .ok_json();
    assert!(
        members.iter().all(|member| member.user_id != leaver),
        "an author is no member"
    );

    let guest = th.add_user("reader").await;
    let guest_id = guest.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest_id}'"
        ))
        .await
        .expect("a guest");
    assert!(
        named(&guest, TEST_WS, &leaver).await.is_empty(),
        "a guest cannot name an unread author"
    );
    th.client
        .add_member(&member(
            &board.id,
            &guest_id,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    assert_eq!(
        named(&guest, TEST_WS, &leaver).await.len(),
        1,
        "a guest reading the contribution names its author"
    );
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM container_members WHERE container_id = '{}'",
            board.id,
        ))
        .await
        .expect("neither reader has a board role");
    for client in [&th.client, &guest] {
        assert!(
            named(client, TEST_WS, &leaver).await.is_empty(),
            "an unread private board reveals no author"
        );
        client
            .get(&format!("{TEST_WS}/users/{leaver}"))
            .await
            .check_not_found();
    }
    th.teardown().await;
}

#[tokio::test]
async fn a_departed_board_creator_is_named_only_to_readers() {
    check_authorship(Authorship::Board).await;
}

#[tokio::test]
async fn a_departed_comment_revision_writer_is_named_only_to_readers() {
    check_authorship(Authorship::CommentVersion).await;
}

#[tokio::test]
async fn a_departed_writer_of_a_readable_trashed_comment_revision_is_named() {
    check_authorship(Authorship::TrashedCommentVersion).await;
}

#[tokio::test]
async fn a_departed_writer_of_a_version_of_a_readable_card_body_is_named() {
    check_authorship(Authorship::BodyVersion).await;
}

#[tokio::test]
async fn a_departed_writer_of_a_version_of_a_readable_page_body_is_named() {
    check_authorship(Authorship::PageVersion).await;
}

#[tokio::test]
async fn a_departed_writer_of_a_version_of_a_readable_file_is_named() {
    check_authorship(Authorship::FileVersion).await;
}
