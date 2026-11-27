//! HTTP contract, format validation, permission boundary, own-space, and cross-org
//! isolation tests for `POST /w/{workspaceId}/archive/import` (`fvoci`, `trello`, `notion`).

use std::io::{Cursor, Write as _};

use fvoci_model::{Container, WorkspaceRole};
use reqwest::StatusCode;
use serde_json::{Value, json};
use zip::write::SimpleFileOptions;

use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TEST_WS, UNKNOWN_ID};
use crate::export::put_part;

/// Opens an import upload named `name`, uploads all parts of `bytes`, and posts to
/// `POST /w/{workspace_id}/archive/import` with optional `format`.
async fn http_import(
    client: &Client,
    workspace_id: &str,
    name: &str,
    bytes: &[u8],
    format: Option<&str>,
) -> ApiResponse {
    let opened = client
        .post(
            &format!("/w/{workspace_id}/archive/uploads"),
            &json!({"name": name, "size": bytes.len()}),
        )
        .await;
    if opened.status != StatusCode::CREATED {
        return opened;
    }
    let upload_id = opened.value()["uploadId"]
        .as_str()
        .expect("uploadId")
        .to_owned();
    let part_size = opened.value()["partSize"]
        .as_u64()
        .and_then(|size| usize::try_from(size).ok())
        .expect("partSize");
    for (index, part) in bytes.chunks(part_size).enumerate() {
        let sent = put_part(client, workspace_id, &upload_id, index + 1, part).await;
        if sent.status != StatusCode::NO_CONTENT {
            return sent;
        }
    }
    let body = match format {
        Some(fmt) => json!({"uploadId": upload_id, "format": fmt}),
        None => json!({"uploadId": upload_id}),
    };
    client
        .post(&format!("/w/{workspace_id}/archive/import"), &body)
        .await
}

fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        writer.start_file(*name, opts).expect("start entry");
        writer.write_all(data).expect("write entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

fn sample_trello_json() -> Vec<u8> {
    json!({
        "id": "tr-http-1",
        "name": "HTTP Trello Board",
        "desc": "Imported over HTTP API.",
        "lists": [
            {"id": "l-open", "name": "Doing", "closed": false, "pos": 1.0},
            {"id": "l-closed", "name": "Archived", "closed": true, "pos": 2.0}
        ],
        "cards": [
            {
                "id": "c-1",
                "name": "Wire API",
                "desc": "Implement endpoint.",
                "idList": "l-open",
                "closed": false,
                "pos": 1.0,
                "attachments": [
                    {"id": "a-1", "name": "Docs", "url": "https://example.com/spec"}
                ]
            },
            {
                "id": "c-2",
                "name": "Old Task",
                "idList": "l-open",
                "closed": true,
                "pos": 2.0
            }
        ],
        "actions": [
            {
                "id": "act-1",
                "type": "commentCard",
                "date": "2026-08-01T12:00:00.000Z",
                "data": {"text": "Ship it!", "card": {"id": "c-1"}},
                "memberCreator": {"fullName": "Reviewer One"}
            }
        ]
    })
    .to_string()
    .into_bytes()
}

fn sample_notion_zip() -> Vec<u8> {
    build_zip(&[
        (
            "Export/Guide 0123456789abcdef0123456789abcdef.md",
            b"# Guide\n\nWelcome to the wiki.\n\n![Logo](Guide%200123456789abcdef0123456789abcdef/logo.png)\n",
        ),
        (
            "Export/Guide 0123456789abcdef0123456789abcdef/logo.png",
            b"\x89PNG\r\n\x1a\npayload",
        ),
        (
            "Export/Tasks 11111111111111111111111111111111.csv",
            b"Name,Status\nFirst Task,Todo\nSecond Task,Done\n",
        ),
    ])
}

#[tokio::test]
async fn http_import_supports_trello_notion_and_default_fvoci_with_summary() {
    let ts = Setup::native().start().await.init_basic().await;

    // 1. Trello JSON over HTTP
    let trello_res = http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "trello-export.json",
        &sample_trello_json(),
        Some("trello"),
    )
    .await;
    trello_res.check_ok();
    let trello_summary = trello_res.value();
    assert_eq!(trello_summary["format"], json!("trello"));
    assert_eq!(trello_summary["imported"]["boards"], json!(1));
    assert_eq!(trello_summary["imported"]["cards"], json!(1));
    assert_eq!(trello_summary["imported"]["comments"], json!(1));
    assert_eq!(trello_summary["membersLeftOut"], json!(0));
    assert_eq!(
        trello_summary["containerIds"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(
        !trello_summary["skipped"]
            .as_array()
            .expect("skipped")
            .is_empty(),
        "closed list/card reported in skipped"
    );
    assert!(
        !trello_summary["warnings"]
            .as_array()
            .expect("warnings")
            .is_empty(),
        "external attachment reported in warnings"
    );

    // 2. Notion ZIP over HTTP
    let notion_res = http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "notion-export.zip",
        &sample_notion_zip(),
        Some("notion"),
    )
    .await;
    notion_res.check_ok();
    let notion_summary = notion_res.value();
    assert_eq!(notion_summary["format"], json!("notion"));
    assert_eq!(notion_summary["imported"]["wikis"], json!(1));
    assert_eq!(notion_summary["imported"]["boards"], json!(1));
    assert_eq!(notion_summary["imported"]["pages"], json!(1));
    assert_eq!(notion_summary["imported"]["cards"], json!(2));
    assert_eq!(notion_summary["imported"]["files"], json!(1));
    assert_eq!(
        notion_summary["containerIds"].as_array().map(Vec::len),
        Some(2)
    );

    // 3. Omitted `format` defaults to `fvoci` (100% backwards-compatible) and reports full counts.
    let trello_board_id = trello_summary["containerIds"][0]
        .as_str()
        .expect("board id");
    let exported = ts
        .client
        .get(&format!(
            "{TEST_WS}/containers/{trello_board_id}/archive/export"
        ))
        .await;
    exported.check_ok();
    let native_res = http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "archive.fvoci",
        &exported.body,
        None,
    )
    .await;
    native_res.check_ok();
    let native_summary = native_res.value();
    assert_eq!(native_summary["format"], json!("fvoci"));
    assert_eq!(native_summary["imported"]["boards"], json!(1));
    assert_eq!(native_summary["imported"]["cards"], json!(1));
    assert_eq!(native_summary["imported"]["comments"], json!(1));
    assert_eq!(native_summary["membersLeftOut"], json!(0));
    assert_eq!(
        native_summary["containerIds"].as_array().map(Vec::len),
        Some(1)
    );

    ts.teardown().await;
}

#[tokio::test]
async fn http_import_rejects_mismatched_extension_empty_upload_and_invalid_format() {
    let ts = Setup::native().start().await.init_basic().await;

    // Extension mismatches (400)
    http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "wrong.zip",
        &sample_trello_json(),
        Some("trello"),
    )
    .await
    .check_bad_request();

    http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "wrong.json",
        &sample_notion_zip(),
        Some("notion"),
    )
    .await
    .check_bad_request();

    http_import(
        &ts.client,
        TEST_WORKSPACE_ID,
        "wrong.txt",
        b"not a zip",
        Some("fvoci"),
    )
    .await
    .check_bad_request();

    // Empty upload (400)
    for (name, fmt) in [
        ("empty.json", Some("trello")),
        ("empty.zip", Some("notion")),
        ("empty.fvoci", None),
    ] {
        http_import(&ts.client, TEST_WORKSPACE_ID, name, b"", fmt)
            .await
            .check_bad_request();
    }

    // Invalid / null format in JSON request body (400)
    let opened = ts
        .client
        .post(
            &format!("{TEST_WS}/archive/uploads"),
            &json!({"name": "board.json", "size": 2}),
        )
        .await;
    opened.check_created();
    let upload_id = opened.value()["uploadId"]
        .as_str()
        .expect("uploadId")
        .to_owned();
    put_part(&ts.client, TEST_WORKSPACE_ID, &upload_id, 1, b"{}")
        .await
        .check_no_content();
    for bad_body in [
        json!({"uploadId": upload_id, "format": "jira"}),
        json!({"uploadId": upload_id, "format": null}),
        json!({"uploadId": upload_id, "format": "trello", "extra": true}),
    ] {
        ts.client
            .post(&format!("{TEST_WS}/archive/import"), &bad_body)
            .await
            .check_bad_request();
    }

    // Format/extension mismatch discards the upload (reusing the same uploadId -> 404).
    ts.client
        .post(
            &format!("{TEST_WS}/archive/import"),
            &json!({"uploadId": upload_id, "format": "notion"}),
        )
        .await
        .check_bad_request();
    ts.client
        .post(
            &format!("{TEST_WS}/archive/import"),
            &json!({"uploadId": upload_id, "format": "trello"}),
        )
        .await
        .check_not_found();

    // Zero containers created after all rejected attempts.
    let boards: Vec<Container> = ts.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let wikis: Vec<Container> = ts.client.get(&format!("{TEST_WS}/wikis")).await.ok_json();
    assert!(boards.is_empty() && wikis.is_empty());

    ts.teardown().await;
}

#[tokio::test]
async fn http_import_enforces_guest_non_member_and_allows_own_space_owner() {
    let ts = Setup::native().start().await.init_basic().await;
    let trello = sample_trello_json();

    // Non-member -> 404 (remove default-workspace membership granted at signup)
    let admin_id = ts.client.me().await.id;
    let outsider = ts.add_user("import-outsider").await;
    let outsider_id = outsider.me().await.id;
    ts.app
        .store()
        .remove_workspace_member(TEST_WORKSPACE_ID, &outsider_id, &admin_id)
        .await
        .expect("remove outsider from default workspace");
    http_import(
        &outsider,
        TEST_WORKSPACE_ID,
        "board.json",
        &trello,
        Some("trello"),
    )
    .await
    .check_not_found();

    // Guest -> 404 (even if posting directly to /archive/import)
    ts.app
        .store()
        .add_workspace_member(TEST_WORKSPACE_ID, &outsider_id, WorkspaceRole::Guest)
        .await
        .expect("add guest");
    http_import(
        &outsider,
        TEST_WORKSPACE_ID,
        "board.json",
        &trello,
        Some("trello"),
    )
    .await
    .check_not_found();
    outsider
        .post(
            &format!("{TEST_WS}/archive/import"),
            &json!({"uploadId": UNKNOWN_ID, "format": "trello"}),
        )
        .await
        .check_not_found();

    // Unknown workspace -> 404
    http_import(
        &ts.client,
        UNKNOWN_ID,
        "board.json",
        &trello,
        Some("trello"),
    )
    .await
    .check_not_found();

    // Personal own-space owner -> 200 OK; another user into that personal space -> 404
    let workspaces: Vec<Value> = ts.client.get("/users/me/workspaces").await.ok_json();
    let own_id = workspaces
        .iter()
        .find(|w| w["kind"] == "personal")
        .and_then(|w| w["id"].as_str())
        .expect("personal workspace")
        .to_owned();

    http_import(&outsider, &own_id, "board.json", &trello, Some("trello"))
        .await
        .check_not_found();

    let own_res = http_import(&ts.client, &own_id, "board.json", &trello, Some("trello")).await;
    own_res.check_ok();
    assert_eq!(own_res.value()["imported"]["boards"], json!(1));

    ts.teardown().await;
}
