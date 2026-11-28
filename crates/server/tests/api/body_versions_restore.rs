//! Going back to a version of a card's body: what is written, what is kept of the
//! body before, what the room is sent, and who may.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::body_versions::{lines, said, versions};
use crate::collab::{body_of, card};
use crate::common::{Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::rooms::{
    Incoming, QUIET, give, lines_of, paragraphs, table, two_in_a_room, write_whole,
};

/// The path that restores a version of a card.
fn restore_path(card_id: &str, version: i64) -> String {
    format!("{TEST_WS}/cards/{card_id}/body-versions/{version}/restore")
}

/// Restores a version of a card as the client's user, and gives the card's lines.
async fn restore(client: &Client, card_id: &str, version: i64) -> Vec<String> {
    let restored: Value = client
        .post_empty(&restore_path(card_id, version))
        .await
        .ok_json();
    assert_eq!(restored["id"], json!(card_id), "the card restored");
    lines_of(restored.get("body").filter(|body| !body.is_null()))
}

/// A version restored while the card is edited together: what the editors typed since
/// is kept as a version of theirs first, the version's body is the card's and its
/// newest version, by who restored it, and everyone in the room is sent it as a new
/// document they go on in. The versions before stay.
#[tokio::test]
async fn a_version_restored_is_sent_to_the_room_and_keeps_what_was_typed() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (user1, user2) = (th.client.me().await.id, th.client2.me().await.id);
    one.write_acked("typed").await;
    two.settle().await;
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(
        said(&kept),
        [(1, Vec::new(), false)],
        "the body before the editors came, which is the card's no more: what they typed is"
    );

    assert_eq!(restore(&th.client2, &t.card_id, 1).await, ["first"]);
    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 2})
        );
        peer.settle().await;
        assert_eq!(
            peer.lines(),
            ["first"],
            "the version, in place of the document"
        );
    }
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(
        said(&kept),
        [
            (3, vec![user2.as_str()], true),
            (2, vec![user1.as_str()], false),
            (1, Vec::new(), false)
        ]
    );
    let causes: Vec<&Value> = kept.iter().map(|version| &version["cause"]).collect();
    assert_eq!(
        causes,
        [&json!("restored"), &json!("auto"), &json!("auto")],
        "the body gone back to, and what the server kept on its own"
    );
    assert_eq!(
        card(&th.client, &t.card_id).await.modified_by,
        user2,
        "written by who restored it"
    );
    assert_eq!(
        lines(&th.client, &t.card_id, &kept[1]).await,
        ["first", "typed"],
        "what was typed can be gone back to"
    );

    // The room goes on in the restored body, and what it types is the next version.
    two.write_acked("after").await;
    one.settle().await;
    assert_eq!(one.lines(), ["first", "after"]);
    assert_eq!(restore(&th.client, &t.card_id, 2).await, ["first", "typed"]);
    for peer in [&mut one, &mut two] {
        peer.text("docReset").await;
        peer.settle().await;
        assert_eq!(peer.lines(), ["first", "typed"]);
    }
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(
        said(&kept)[..3],
        [
            (5, vec![user1.as_str()], true),
            (4, vec![user2.as_str()], false),
            (3, vec![user2.as_str()], false)
        ]
    );
    assert_eq!(
        lines(&th.client, &t.card_id, &kept[1]).await,
        ["first", "after"]
    );
    one.leave().await;
    two.leave().await;
    crate::rooms::no_rooms(&th).await;
    assert_eq!(
        versions(&th.client, &t.card_id).await,
        kept,
        "left as it was"
    );
    th.teardown().await;
}

/// A version restored with nobody in the room is the card's body, and no body is
/// restored as none. Restoring the body the card has changes nothing: not the card,
/// not its versions, and the room is sent nothing.
#[tokio::test]
async fn a_version_restored_is_the_cards_body_and_the_same_body_again_is_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user1 = th.client.me().await.id;
    write_whole(&th.client, &t.card_id, &paragraphs(&["second"])).await;
    let read = card(&th.client, &t.card_id).await;
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", t.card_id),
            &json!({"body": null, "readAt": read.updated_at}),
        )
        .await
        .check_ok();

    let mut listener = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &user1).await;
    assert_eq!(restore(&th.client, &t.card_id, 1).await, ["first"]);
    let told = listener
        .expect("blockChanged", "/block/id", &t.card_id)
        .await;
    assert_eq!(told["block"]["modifiedBy"], json!(user1), "{told}");
    assert_eq!(
        Some(lines_of(
            body_of(&card(&th.client, &t.card_id).await).as_ref()
        )),
        Some(vec!["first".to_owned()])
    );
    assert_eq!(
        said(&versions(&th.client, &t.card_id).await),
        [
            (4, vec![user1.as_str()], true),
            (3, vec![user1.as_str()], false),
            (2, vec![user1.as_str()], false),
            (1, Vec::new(), false)
        ],
        "the body before was a version already; the body restored is the newest"
    );
    assert_eq!(
        restore(&th.client, &t.card_id, 3).await,
        Vec::<String>::new(),
        "no body"
    );
    assert_eq!(body_of(&card(&th.client, &t.card_id).await), None);
    listener
        .expect("blockChanged", "/block/id", &t.card_id)
        .await;

    // The body the card has, by its newest version and by an older one.
    let before = card(&th.client, &t.card_id).await;
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(kept.len(), 5, "{kept:?}");
    let mut reader = crate::rooms::Peer::connect(&th.client, 101).await;
    reader.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    for same in [5, 3] {
        assert_eq!(
            restore(&th.client, &t.card_id, same).await,
            Vec::<String>::new()
        );
    }
    assert_eq!(versions(&th.client, &t.card_id).await, kept);
    assert_eq!(
        card(&th.client, &t.card_id).await.updated_at,
        before.updated_at,
        "the card is not written"
    );
    assert_eq!(reader.incoming(QUIET).await, Incoming::Nothing);
    let heard = listener.drain().await;
    assert!(
        heard
            .iter()
            .all(|message| message["type"] != json!("blockChanged")),
        "a restore that writes nothing tells nobody: {heard:?}"
    );
    th.teardown().await;
}

/// The oldest version a server keeps is restored as any other: it goes as the body
/// restored comes as the newest, and is the card's body. A version that shows an image
/// is restored with it, while the board has its file.
#[tokio::test]
async fn the_oldest_version_and_one_with_an_image_are_restored() {
    oldest_and_one_with_a_file_restored("image").await;
}

#[tokio::test]
async fn the_oldest_version_and_one_with_a_file_block_are_restored() {
    oldest_and_one_with_a_file_restored("file").await;
}

async fn oldest_and_one_with_a_file_restored(kind: &str) {
    let th = Setup::native()
        .configure(|config| config.collab.collab_body_versions = 2)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user1 = th.client.me().await.id;
    let uploaded = crate::files::upload(&th.client, &t.board_id, "picture.png", b"png").await;
    let file_id = crate::files::uploaded_id(&uploaded);
    let pictured = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "pictured"}]},
        {"type": kind, "attrs": {"fileId": file_id}},
    ]});
    write_whole(&th.client, &t.card_id, &pictured).await;
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(said(&kept).len(), 2, "{kept:?}");

    assert_eq!(
        restore(&th.client, &t.card_id, 1).await,
        ["first"],
        "{kind}"
    );
    assert_eq!(
        said(&versions(&th.client, &t.card_id).await),
        [
            (3, vec![user1.as_str()], true),
            (2, vec![user1.as_str()], false)
        ],
        "the version restored went, and its body is the newest"
    );
    th.client
        .post_empty(&restore_path(&t.card_id, 1))
        .await
        .check_error(StatusCode::NOT_FOUND, "version not found");

    let restored: Value = th
        .client
        .post_empty(&restore_path(&t.card_id, 2))
        .await
        .ok_json();
    assert_eq!(restored["body"], pictured, "with its image");
    assert_eq!(
        said(&versions(&th.client, &t.card_id).await),
        [
            (4, vec![user1.as_str()], true),
            (3, vec![user1.as_str()], false)
        ],
        "{kind}"
    );
    th.teardown().await;
}

/// A version whose body may not be stored any more, and one that shows an image
/// whose file the board does not have, are not gone back to (409): the card keeps
/// its body, and its versions are as they were.
#[tokio::test]
async fn a_version_that_cannot_be_stored_is_not_restored() {
    version_that_cannot_be_stored("image").await;
}

#[tokio::test]
async fn a_version_whose_file_block_has_no_file_is_not_restored() {
    version_that_cannot_be_stored("file").await;
}

async fn version_that_cannot_be_stored(kind: &str) {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let second = paragraphs(&["second"]);
    write_whole(&th.client, &t.card_id, &second).await;
    let kept = versions(&th.client, &t.card_id).await;
    assert_eq!(said(&kept).len(), 2, "{kept:?}");
    let unknown = json!({"type": "doc", "content": [{"type": "marquee"}]});
    let pictured = json!({"type": "doc", "content": [
        {"type": kind, "attrs": {"fileId": "7missing.png"}},
    ]});
    for (body, refusal) in [
        (
            unknown,
            "the body of the version may not be stored any more: invalid body node: node type \
             is not on the allowlist",
        ),
        (
            pictured,
            "the version names a file (of an image or a file block) that is not there any \
             more",
        ),
    ] {
        // As a version kept before the server was told to store less, or whose file
        // was lost: nothing the server does today writes one.
        let planted = th
            .app
            .store()
            .execute_unchecked(&format!(
                "UPDATE body_versions SET body = '{body}' WHERE block_id = '{}' AND version = 1",
                t.card_id
            ))
            .await
            .expect("the version is written over");
        assert_eq!(planted, 1, "{kind}");
        th.client
            .post_empty(&restore_path(&t.card_id, 1))
            .await
            .check_error(StatusCode::CONFLICT, refusal);
        assert_eq!(
            body_of(&card(&th.client, &t.card_id).await).as_ref(),
            Some(&second),
            "{kind}"
        );
        assert_eq!(versions(&th.client, &t.card_id).await.len(), 2, "{kind}");
    }
    th.teardown().await;
}

/// Only who may edit the card restores a version of it: not a viewer or a commenter
/// of its board (403), not who is no member of it or asks through another workspace
/// (no such card), and not a number the card has no version of.
#[tokio::test]
async fn only_who_edits_the_card_restores_a_version() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    write_whole(&th.client, &t.card_id, &paragraphs(&["second"])).await;
    let kept = versions(&th.client, &t.card_id).await;
    let path = restore_path(&t.card_id, 1);

    th.client2
        .post_empty(&path)
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    th.anonymous().post_empty(&path).await.check_unauthorized();
    th.client
        .post_empty(&path.replace(TEST_WS, ROOT_WS))
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    for role in ["viewer", "commenter"] {
        give(&th, &t.board_id, &user2, role).await;
        th.client2.post_empty(&path).await.check_forbidden();
        assert_eq!(
            versions(&th.client2, &t.card_id).await,
            kept,
            "a {role} reads them"
        );
    }
    for missing in ["9", "0", "two"] {
        th.client
            .post_empty(&path.replace("/1/restore", &format!("/{missing}/restore")))
            .await
            .check_error(StatusCode::NOT_FOUND, "version not found");
    }
    assert_eq!(
        versions(&th.client, &t.card_id).await,
        kept,
        "nothing restored"
    );

    give(&th, &t.board_id, &user2, "editor").await;
    assert_eq!(restore(&th.client2, &t.card_id, 1).await, ["first"]);
    assert_eq!(
        said(&versions(&th.client, &t.card_id).await)[0],
        (3, vec![user2.as_str()], true)
    );

    // Not of a card in the trash.
    crate::blocks::trash_card(&th.client, &t.board_id, &t.card_id)
        .await
        .check_no_content();
    th.client
        .post_empty(&path)
        .await
        .check_error(StatusCode::NOT_FOUND, "card not found");
    th.teardown().await;
}
