//! A container exported as Markdown files (`exportContainerMarkdown`): a wiki as the
//! folders of its tree, a board as its cards and a table of them, a course as its
//! chapters' folders, with the files they show and what they name linked where the ZIP
//! puts it; and a page or a unit read in Markdown (`format=markdown`) as a card is.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};

use fvoci_model::{Card, Container, ContainerVisibility};
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::files::{upload, uploaded_id};
use crate::wiki_copies::{page, wiki, write};

/// The entries of a ZIP, by name, as text.
fn entries(response: &ApiResponse) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(response.body.clone())).expect("a ZIP");
    let mut entries = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).expect("an entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("its bytes");
        entries.insert(
            entry.name().expect("a UTF-8 name").into_owned(),
            String::from_utf8_lossy(&bytes).into_owned(),
        );
    }
    entries
}

/// The Markdown export of `container_id`, as `client` asks for it.
async fn exported(client: &Client, container_id: &str) -> ApiResponse {
    client
        .get(&format!(
            "{TEST_WS}/containers/{container_id}/markdown/export"
        ))
        .await
}

fn header<'a>(response: &'a ApiResponse, name: &str) -> &'a str {
    response
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

/// A wiki is a folder of its tree: a page with pages below it has a folder of its name
/// beside its file, the files its bodies show are in `files/`, and a reference is a
/// link where the ZIP holds what it names, its title where the reader reads it
/// elsewhere; a person is named in plain text. The trash stays behind.
#[tokio::test]
async fn a_wiki_is_exported_as_the_folders_of_its_tree() {
    let ts = Setup::native().start().await.init_basic().await;
    let me = ts.client.me().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let elsewhere: Card = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Elsewhere"}),
        )
        .await
        .created_json();
    let wiki = wiki(&ts.client, "Handbook").await;
    let welcome = page(&ts.client, &wiki, "", "Welcome").await;
    let started = page(&ts.client, &wiki, &welcome, "Getting started: day one").await;
    page(&ts.client, &wiki, "", "Policies").await;
    let old = page(&ts.client, &wiki, "", "Old").await;
    ts.client
        .delete(&format!("{TEST_WS}/wikis/{wiki}/pages/{old}"))
        .await
        .check_no_content();
    let picture = uploaded_id(&upload(&ts.client, &wiki, "moon pic.png", b"a picture\n").await);
    let minutes = uploaded_id(&upload(&ts.client, &wiki, "minutes.pdf", b"%PDF minutes\n").await);
    let shown = json!({"type": "doc", "content": [
        {"type": "image", "attrs": {"fileId": picture}},
        {"type": "file", "attrs": {"fileId": minutes}},
    ]});
    write(&ts.client, &wiki, &started, &shown).await;
    let naming = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "see "},
        {"type": "reference", "attrs": {"id": started}},
        {"type": "text", "text": " and "},
        {"type": "reference", "attrs": {"id": elsewhere.id}},
        {"type": "text", "text": ", ask "},
        {"type": "mention", "attrs": {"userId": me.id}},
    ]}]});
    write(&ts.client, &wiki, &welcome, &naming).await;

    let response = exported(&ts.client, &wiki).await;
    response.check_ok();
    assert_eq!(header(&response, "content-type"), "application/zip");
    let disposition = header(&response, "content-disposition");
    let day = disposition
        .strip_prefix("attachment; filename=markdown-")
        .and_then(|rest| rest.strip_suffix(".zip"));
    assert!(day.is_some_and(|day| day.len() == 10), "{disposition}");
    let files = entries(&response);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "Handbook/Policies.md",
            "Handbook/Welcome.md",
            "Handbook/Welcome/Getting started_ day one.md",
            "Handbook/files/minutes.pdf",
            "Handbook/files/moon pic.png",
        ]
    );
    assert_eq!(files["Handbook/files/moon pic.png"], "a picture\n");
    let welcome_md = &files["Handbook/Welcome.md"];
    assert!(
        welcome_md.starts_with("---\ntitle: \"Welcome\"\n"),
        "{welcome_md}"
    );
    assert!(
        welcome_md.contains("\n---\n\n# Welcome\n\n"),
        "{welcome_md}"
    );
    let shown_name = if me.display_name.is_empty() {
        &me.username
    } else {
        &me.display_name
    };
    assert!(
        welcome_md.contains(&format!(
            "see [Getting started: day one](<Welcome/Getting started_ day one.md>) and \
             Elsewhere, ask @{shown_name}"
        )),
        "{welcome_md}"
    );
    let started_md = &files["Handbook/Welcome/Getting started_ day one.md"];
    assert!(
        started_md.contains("](<../files/moon pic.png>)")
            && started_md.contains("[minutes.pdf](<../files/minutes.pdf>)"),
        "{started_md}"
    );
    ts.teardown().await;
}

/// A board is its cards by number, templates aside, each with its number and
/// attachments in its front matter, and `board.csv` of them under its properties.
#[tokio::test]
async fn a_board_is_exported_as_its_cards_and_their_table() {
    let ts = Setup::native().start().await.init_basic().await;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Road map",
                "cardProperties": [{
                    "id": "p1", "name": "Status", "type": "select",
                    "options": [{"id": "open", "label": "Open", "color": "default"}],
                }],
            }),
        )
        .await
        .created_json();
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    let first: Card = ts
        .client
        .post(
            &cards,
            &json!({"title": "First", "properties": {"p1": "open"}}),
        )
        .await
        .created_json();
    ts.client
        .post(&cards, &json!({"title": "=Second, with a comma"}))
        .await
        .check_created();
    ts.client
        .post(&cards, &json!({"title": "A template", "isTemplate": true}))
        .await
        .check_created();
    let file = uploaded_id(&upload(&ts.client, &board.id, "plan.txt", b"the plan\n").await);
    ts.client
        .post(
            &format!("{cards}/{}/attachments", first.id),
            &json!({"fileId": file}),
        )
        .await
        .check_created();

    let response = exported(&ts.client, &board.id).await;
    response.check_ok();
    let files = entries(&response);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "Road map/board.csv",
            "Road map/cards/1-First.md",
            "Road map/cards/2-=Second, with a comma.md",
            "Road map/files/plan.txt",
        ]
    );
    let first_md = &files["Road map/cards/1-First.md"];
    assert!(
        first_md.starts_with("---\ntitle: \"First\"\nnumber: \"1\"\n"),
        "{first_md}"
    );
    assert!(
        first_md.contains("\nattachments: [\"../files/plan.txt\"]\n"),
        "{first_md}"
    );
    assert_eq!(
        files["Road map/board.csv"],
        "\u{feff}Number,Title,Status\r\n1,First,Open\r\n2,\"'=Second, with a comma\",\r\n"
    );
    ts.teardown().await;
}

/// A board's options and files are no cards: only its relations are asked for the cards
/// they name, by their links, and the table calls each by its title.
#[tokio::test]
async fn a_board_names_its_related_cards_and_takes_options_and_files_for_none() {
    let ts = Setup::native().start().await.init_basic().await;
    let board: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Plan",
                "cardProperties": [
                    {"id": "tags", "name": "Tags", "type": "multiSelect",
                     "options": [{"id": "urgent", "label": "Urgent", "color": "default"}]},
                    {"id": "docs", "name": "Docs", "type": "file"},
                    {"id": "after", "name": "After", "type": "relation"},
                ],
            }),
        )
        .await
        .created_json();
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);
    let file = uploaded_id(&upload(&ts.client, &board.id, "brief.txt", b"the brief\n").await);
    let first: Card = ts
        .client
        .post(
            &cards,
            &json!({"title": "First", "properties": {"tags": ["urgent"], "docs": [file]}}),
        )
        .await
        .created_json();
    ts.client
        .post(
            &cards,
            &json!({"title": "Second", "properties": {"after": [{"card": first.id}]}}),
        )
        .await
        .check_created();

    let response = exported(&ts.client, &board.id).await;
    response.check_ok();
    assert_eq!(
        entries(&response)["Plan/board.csv"],
        "\u{feff}Number,Title,Tags,Docs,After\r\n1,First,Urgent,brief.txt,\r\n2,Second,,,First\r\n"
    );
    ts.teardown().await;
}

/// A course is its chapters' folders of their units, each unit with its amount and its
/// day; and a page or a unit read with `format=markdown` has its body in Markdown in
/// place of it, as a card has.
#[tokio::test]
async fn a_course_is_exported_by_chapters_and_its_units_read_in_markdown() {
    let ts = Setup::native().start().await.init_basic().await;
    let course: Container = ts
        .client
        .post(
            &format!("{TEST_WS}/courses"),
            &json!({"visibility": "open", "title": "Systems"}),
        )
        .await
        .created_json();
    let units = format!("{TEST_WS}/courses/{}/units", course.id);
    let chapter = ts
        .client
        .post(&units, &json!({"kind": "chapter", "title": "Foundations"}))
        .await
        .created_json::<Value>()["id"]
        .as_str()
        .expect("a chapter")
        .to_owned();
    let unit = ts
        .client
        .post(
            &units,
            &json!({
                "parentId": chapter, "title": "Clocks", "amount": 60,
                "amountUnit": "minutes", "scheduledDate": "2026-12-03",
            }),
        )
        .await
        .created_json::<Value>()["id"]
        .as_str()
        .expect("a unit")
        .to_owned();
    let path = format!("{units}/{unit}");
    let read_at = ts.client.get(&path).await.value()["updatedAt"].clone();
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "tick "},
        {"type": "text", "text": "tock", "marks": [{"type": "bold"}]},
    ]}]});
    ts.client
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();

    let read: Value = ts
        .client
        .get(&format!("{path}?format=markdown"))
        .await
        .ok_json();
    assert_eq!(read["bodyMarkdown"], "tick **tock**");
    assert_eq!(read["hasComplexBlocks"], false);
    assert_eq!(read["body"], Value::Null, "in place of the body: {read}");
    ts.client
        .get(&format!("{path}?format=html"))
        .await
        .check_bad_request();

    let files = entries(&exported(&ts.client, &course.id).await);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        ["Systems/Foundations/Clocks.md"]
    );
    let clocks = &files["Systems/Foundations/Clocks.md"];
    assert!(
        clocks.starts_with(
            "---\ntitle: \"Clocks\"\namount: \"60\"\namountUnit: \"minutes\"\n\
             scheduledDate: \"2026-12-03\"\n"
        ),
        "{clocks}"
    );
    assert!(clocks.ends_with("# Clocks\n\ntick **tock**\n"), "{clocks}");

    let wiki = wiki(&ts.client, "Notes").await;
    let note = page(&ts.client, &wiki, "", "Note").await;
    write(&ts.client, &wiki, &note, &body).await;
    let read: Value = ts
        .client
        .get(&format!(
            "{TEST_WS}/wikis/{wiki}/pages/{note}?format=markdown"
        ))
        .await
        .ok_json();
    assert_eq!(read["bodyMarkdown"], "tick **tock**");
    assert_eq!(read["body"], Value::Null, "in place of the body: {read}");
    ts.teardown().await;
}
