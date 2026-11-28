//! The authors an archive names: everything imported is written as the importer's, so
//! an archive can make nobody an author: not somebody the importer does not see, whom
//! the workspace could then name, and not a colleague, in whose name it could speak.

use std::io::{Cursor, Write as _};

use fvoci_model::{BlockType, Container, User, new_id};
use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::export::import;

/// An archive of one board, whose lines are `lines`.
fn archive(lines: &[Value]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("authored/container.jsonl", options)
        .expect("start");
    for line in lines {
        writer
            .write_all(format!("{line}\n").as_bytes())
            .expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn block(id: &str, kind: &str, created_by: &str, modified_by: &str) -> Value {
    json!({"type": "block", "data": {
        "id": id, "containerId": "board", "type": kind, "title": id,
        "createdBy": created_by, "modifiedBy": modified_by, "createdAt": 1, "updatedAt": 1,
    }})
}

/// A user who is no member of the organization, nor of any of its workspaces.
async fn outsider(th: &TestServer) -> String {
    let mut user = User {
        id: new_id(),
        username: "outsider".to_owned(),
        email: "outsider@sample.com".to_owned(),
        ..User::default()
    };
    th.app
        .store()
        .create_user(&mut user)
        .await
        .expect("the outsider is stored");
    for table in ["workspace_members", "organization_members"] {
        th.app
            .store()
            .execute_unchecked(&format!(
                "DELETE FROM {table} WHERE user_id = '{}'",
                user.id
            ))
            .await
            .expect("the outsider leaves");
    }
    user.id
}

async fn named(th: &TestServer, user: &str) -> Vec<User> {
    th.client
        .post(&format!("{TEST_WS}/users"), &[user])
        .await
        .ok_json()
}

#[tokio::test]
async fn everything_imported_is_the_importers_whoever_the_archive_names() {
    let th = Setup::native().start().await.init_basic().await;
    let importer = th.client.me().await.id;
    let colleague = th.client2.me().await.id;
    let outsider = outsider(&th).await;
    // A member of the organization in its default workspace only: not in this one.
    let elsewhere = th.stranger().await;
    assert!(
        named(&th, &outsider).await.is_empty(),
        "nobody names the outsider before the import"
    );
    assert!(
        named(&th, &elsewhere).await.is_empty(),
        "the importer does not see a member of another workspace"
    );
    assert_eq!(
        named(&th, &colleague).await.len(),
        1,
        "the colleague works in the workspace"
    );
    let comment = json!({"type": "block", "data": {
        "id": "comment", "containerId": "board", "parentId": "theirs", "type": "comment",
        "title": "", "createdBy": outsider, "modifiedBy": outsider, "createdAt": 1,
        "updatedAt": 1,
        "body": {"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "hello"}]}]},
    }});
    let bytes = archive(&[
        json!({"type": "container", "data": {
            "id": "board", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Authored",
            "createdBy": outsider, "modifiedBy": colleague, "createdAt": 1, "updatedAt": 1,
        }}),
        block("theirs", "card", &outsider, &outsider),
        block("elsewhere", "card", &elsewhere, &elsewhere),
        block("ours", "card", &colleague, &colleague),
        block("mixed", "card", &colleague, &outsider),
        block("nobody", "card", "", ""),
        comment,
    ]);
    import(&th.client, TEST_WORKSPACE_ID, &bytes)
        .await
        .check_ok();

    let boards: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let board = boards
        .into_iter()
        .find(|board| board.title == "Authored")
        .expect("the imported board");
    assert_eq!(board.created_by, importer, "the board's outside creator");
    assert_eq!(
        board.modified_by, importer,
        "the board's colleague modifier"
    );
    let blocks = crate::blocks::blocks_of(&th.client, &board.id).await;
    let by_title = |title: &str| {
        blocks
            .iter()
            .find(|block| block.title == title)
            .unwrap_or_else(|| panic!("the block {title}"))
    };
    for title in ["theirs", "elsewhere", "ours", "mixed", "nobody"] {
        let block = by_title(title);
        assert_eq!(block.created_by, importer, "{title}'s creator");
        assert_eq!(block.modified_by, importer, "{title}'s modifier");
    }
    let comment = blocks
        .iter()
        .find(|block| block.block_type == BlockType::Comment)
        .expect("the imported comment");
    assert_eq!(comment.created_by, importer, "the comment's outside writer");
    assert_eq!(comment.modified_by, importer);

    assert!(
        named(&th, &outsider).await.is_empty(),
        "the outsider is still named to nobody"
    );
    assert!(
        named(&th, &elsewhere).await.is_empty(),
        "the member of another workspace is still named to nobody here"
    );
    th.teardown().await;
}
