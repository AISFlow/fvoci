//! Courses through HTTP, roles, collaborative rooms, and archive export/import.

use std::io::{Cursor, Write as _};
use std::time::Duration;

use fvoci_model::{CHAPTERS_PER_COURSE, ContainerKind, UNITS_PER_COURSE, Unit, UnitKind, UnitNode};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::boards::another_workspace;
use crate::common::{Client, PASSWORD, Setup, TEST_WORKSPACE_ID, TEST_WS, USER1_USERNAME};
use crate::export::import;
use crate::files::{upload, uploaded_id};
use crate::rooms::{PATIENCE, Peer, give, lines_of, paragraphs};

async fn create_course(client: &Client, ws: &str, course_type: &str, title: &str) -> String {
    let made = client
        .post(
            &format!("{ws}/courses"),
            &json!({"visibility": course_type, "title": title}),
        )
        .await;
    made.check_created();
    made.value()["id"].as_str().expect("course id").to_owned()
}

async fn create_item(
    client: &Client,
    ws: &str,
    course_id: &str,
    kind: &str,
    parent_id: &str,
    title: &str,
) -> Unit {
    let made = client
        .post(
            &format!("{ws}/courses/{course_id}/units"),
            &json!({"kind": kind, "parentId": parent_id, "title": title}),
        )
        .await;
    made.check_created();
    made.created_json()
}

async fn write_unit_body(client: &Client, path: &str, read_at: i64, body: &Value) -> Unit {
    client
        .patch(path, &json!({"readAt": read_at, "body": body}))
        .await
        .ok_json()
}

fn body_of(unit: &Unit) -> Option<Value> {
    unit.body.as_ref().map(|body| body.document().clone())
}

async fn unit_holds(client: &Client, path: &str, body: &Value) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let held = body_of(&client.get(path).await.ok_json());
        if held.as_ref() == Some(body) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the unit holds {held:?}, not {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Full HTTP outline lifecycle: chapters, units, bodies, conflict checks, moves, trash,
/// undelete, and problem documentation pages.
#[tokio::test]
async fn course_outline_crud_move_trash_and_problem_pages() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let client = &th.client;
    let course = create_course(client, TEST_WS, "private", "Rust 101").await;
    let listed: Vec<Value> = client.get(&format!("{TEST_WS}/courses")).await.ok_json();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (&listed[0]["id"], &listed[0]["kind"], &listed[0]["title"]),
        (&json!(course), &json!("course"), &json!("Rust 101"))
    );

    let ch1 = create_item(client, TEST_WS, &course, "chapter", "", "Week 1").await;
    let ch2 = create_item(client, TEST_WS, &course, "chapter", "", "Week 2").await;
    let u1 = create_item(client, TEST_WS, &course, "unit", &ch1.id, "Ownership").await;
    let u2 = create_item(client, TEST_WS, &course, "unit", &ch1.id, "Borrowing").await;
    assert_eq!((ch1.kind, u1.kind), (UnitKind::Chapter, UnitKind::Unit));

    // Patch unit title and body; stale readAt returns 409; chapter refuses body.
    let u1_path = format!("{TEST_WS}/courses/{course}/units/{}", u1.id);
    let updated = write_unit_body(client, &u1_path, u1.updated_at, &paragraphs(&["hello"])).await;
    assert_eq!(body_of(&updated), Some(paragraphs(&["hello"])));
    let stale = client
        .patch(
            &u1_path,
            &json!({"readAt": u1.updated_at, "body": paragraphs(&["stale"])}),
        )
        .await;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.text());

    for bad_body in [
        json!({"body": paragraphs(&["nope"])}),
        json!({"body": null, "readAt": ch1.updated_at}),
    ] {
        let ch_body = client
            .patch(
                &format!("{TEST_WS}/courses/{course}/units/{}", ch1.id),
                &bad_body,
            )
            .await;
        ch_body.check_bad_request();
        assert_eq!(ch_body.refused_at(), "/body", "{}", ch_body.text());
    }

    // Move u2 into ch2 and check the outline.
    client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{}/move", u2.id),
            &json!({"parentId": ch2.id}),
        )
        .await
        .check_ok();
    let tree: Vec<UnitNode> = client
        .get(&format!("{TEST_WS}/courses/{course}/units"))
        .await
        .ok_json();
    assert_eq!(tree.len(), 4);
    assert_eq!(
        tree.iter()
            .find(|n| n.id == u2.id)
            .map(|n| n.parent_id.as_str()),
        Some(ch2.id.as_str())
    );

    // Deleting chapter 1 deletes u1 with it; trash lists ch1 with childCount = 1.
    client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{}", ch1.id))
        .await
        .check_no_content();
    let trash: Vec<Value> = client
        .get(&format!("{TEST_WS}/courses/{course}/trash/units"))
        .await
        .ok_json();
    assert_eq!(trash.len(), 1);
    assert_eq!(
        (&trash[0]["id"], &trash[0]["kind"], &trash[0]["below"]),
        (&json!(ch1.id), &json!("chapter"), &json!(1))
    );

    // Undelete chapter 1 -> both ch1 and u1 come back.
    client
        .post(
            &format!("{TEST_WS}/courses/{course}/units/{}/restore", ch1.id),
            &json!({}),
        )
        .await
        .check_ok();
    let read_u1: Unit = client.get(&u1_path).await.ok_json();
    assert_eq!(read_u1.parent_id, ch1.id);

    // Delete u2 separately, then delete ch2, then undelete u2 -> top level.
    let u2_path = format!("{TEST_WS}/courses/{course}/units/{}", u2.id);
    client.delete(&u2_path).await.check_no_content();
    client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{}", ch2.id))
        .await
        .check_no_content();
    let restored: Unit = client
        .post(&format!("{u2_path}/restore"), &json!({}))
        .await
        .ok_json();
    assert_eq!(restored.parent_id, "");

    // Problem documentation pages exist for both limits (served at the server root).
    let http = reqwest::Client::new();
    for path in ["/problems/too-many-units", "/problems/too-many-chapters"] {
        let page = http
            .get(format!("{}{path}", th.base_url))
            .send()
            .await
            .expect("problem page fetched");
        assert_eq!(page.status(), StatusCode::OK, "{path}");
        let body = page.text().await.expect("problem text");
        assert!(body.contains(path), "{path}: {body}");
    }
    th.teardown().await;
}

/// Open courses are read and searched without joining by workspace members (not guests),
/// viewers get read-only collab rooms, guests see only invited courses, and removed
/// members are evicted from open unit rooms.
#[tokio::test]
async fn open_course_and_workspace_roles_control_access_search_and_rooms() {
    let th = Setup::native().start().await.init_basic().await;

    // 1) Open course: unjoined member reads, searches, and gets a read-only room.
    let open = create_course(&th.client, TEST_WS, "open", "Open Course").await;
    let open_unit = create_item(
        &th.client,
        TEST_WS,
        &open,
        "unit",
        "",
        "OpenSyllableKeyword",
    )
    .await;
    let open_units = format!("{TEST_WS}/courses/{open}/units");
    let open_unit_path = format!("{open_units}/{}", open_unit.id);
    write_unit_body(
        &th.client,
        &open_unit_path,
        open_unit.updated_at,
        &paragraphs(&["open lesson body"]),
    )
    .await;

    th.client2.get(&open_units).await.check_ok();
    th.client2.get(&open_unit_path).await.check_ok();
    th.client2
        .patch(&open_unit_path, &json!({"title": "no write"}))
        .await
        .check_forbidden();
    let found: Value = th
        .client2
        .get(&format!("{TEST_WS}/search?q=OpenSyllableKeyword&kind=unit"))
        .await
        .ok_json();
    assert_eq!(found[0]["id"], json!(open_unit.id), "{found}");
    assert_eq!(found[0]["kind"], json!("unit"), "{found}");
    assert_eq!(found[0]["containerKind"], json!("course"), "{found}");

    let mut unjoined = Peer::connect(&th.client2, 301).await;
    let joined = unjoined.joined(TEST_WORKSPACE_ID, &open_unit.id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    unjoined.write("unjoined cannot write").await;
    let nack = unjoined.text("docNack").await;
    assert_eq!(nack["reason"], json!("document_read_only"), "{nack}");
    unjoined.leave().await;

    // Join gives default `commenter` role (comments allowed, unit patch forbidden);
    // raising `minimumRole` to `editor` lets joiners edit; leaving returns to read-only.
    let join_res: Value = th
        .client2
        .post(&format!("{TEST_WS}/containers/{open}/join"), &json!({}))
        .await
        .ok_json();
    assert_eq!(join_res["role"], json!("commenter"));
    th.client2
        .patch(&open_unit_path, &json!({"title": "still commenter"}))
        .await
        .check_forbidden();
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{open}"),
            &json!({"minimumRole": "editor"}),
        )
        .await
        .check_ok();
    th.client2
        .post(&format!("{TEST_WS}/containers/{open}/join"), &json!({}))
        .await
        .check_ok();
    th.client2
        .patch(&open_unit_path, &json!({"title": "OpenSyllableKeyword"}))
        .await
        .check_ok();
    th.client2
        .post(&format!("{TEST_WS}/containers/{open}/leave"), &json!({}))
        .await
        .check_no_content();
    th.client2
        .patch(&open_unit_path, &json!({"title": "left"}))
        .await
        .check_forbidden();

    th.teardown().await;
}

/// Viewers get read-only collab rooms, removed members are evicted from open unit rooms,
/// and guests see only the course they were invited to.
#[tokio::test]
async fn course_roles_and_guest_isolation_control_rooms_and_lists() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;
    let open = create_course(&th.client, TEST_WS, "open", "Open Course").await;
    let open_unit = create_item(
        &th.client,
        TEST_WS,
        &open,
        "unit",
        "",
        "OpenSyllableKeyword",
    )
    .await;
    let open_unit_path = format!("{TEST_WS}/courses/{open}/units/{}", open_unit.id);

    // Private course: viewer gets read-only room; removal kicks from room.
    let private = create_course(&th.client, TEST_WS, "private", "Private Course").await;
    let p_unit = create_item(&th.client, TEST_WS, &private, "unit", "", "Lesson 1").await;
    let p_path = format!("{TEST_WS}/courses/{private}/units/{}", p_unit.id);
    write_unit_body(
        &th.client,
        &p_path,
        p_unit.updated_at,
        &paragraphs(&["start"]),
    )
    .await;

    give(&th, &private, &user2, "viewer").await;
    let mut peer2 = Peer::connect(&th.client2, 302).await;
    let joined = peer2.joined(TEST_WORKSPACE_ID, &p_unit.id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    peer2.write("viewer blocked").await;
    let nack = peer2.text("docNack").await;
    assert_eq!(nack["reason"], json!("document_read_only"), "{nack}");
    peer2.joined(TEST_WORKSPACE_ID, &p_unit.id).await;

    // Removing user2 from the course evicts them from the room.
    th.client
        .delete(&format!("{TEST_WS}/containers/{private}/members/{user2}"))
        .await
        .check_no_content();
    let left = peer2.text("docLeft").await;
    assert_eq!(left["reason"], json!("not_found"), "{left}");
    let refused = peer2.join(TEST_WORKSPACE_ID, &p_unit.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");

    // 3) Guest sees ONLY the invited course (not the open course, not uninvited private ones).
    let guest_id = crate::guests::guest_of_the_workspace(&th).await;
    let invited = create_course(&th.client, TEST_WS, "private", "Invited Course").await;
    let inv_unit = create_item(&th.client, TEST_WS, &invited, "unit", "", "InvitedUnit").await;
    crate::references::let_read(&th.client, &invited, &guest_id).await;

    let courses: Vec<Value> = th
        .client2
        .get(&format!("{TEST_WS}/courses"))
        .await
        .ok_json();
    let ids: Vec<&str> = courses.iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(ids, [invited.as_str()], "guest sees only invited course");
    th.client2.get(&open_unit_path).await.check_not_found();
    th.client2
        .get(&format!(
            "{TEST_WS}/courses/{invited}/units/{}",
            inv_unit.id
        ))
        .await
        .check_ok();
    let guest_search: Value = th
        .client2
        .get(&format!("{TEST_WS}/search?q=OpenSyllableKeyword"))
        .await
        .ok_json();
    assert_eq!(guest_search, json!([]), "{guest_search}");
    let mut guest_peer = Peer::connect(&th.client2, 303).await;
    let refused = guest_peer.join(TEST_WORKSPACE_ID, &open_unit.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    let ok = guest_peer.joined(TEST_WORKSPACE_ID, &inv_unit.id).await;
    assert_eq!(ok["mode"], json!("read"), "{ok}");
    guest_peer.leave().await;

    th.teardown().await;
}

/// Two connections co-edit one unit via `joinDoc`:
/// (a) two users in a shared workspace, and
/// (b) two sessions of one account in their personal space.
/// Chapters refuse `joinDoc` with `not_found`.
#[tokio::test]
async fn two_connections_coedit_a_unit_in_workspace_and_in_own_space() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;

    // (a) Shared workspace: two users co-edit a unit; chapter refuses joinDoc.
    let course = create_course(&th.client, TEST_WS, "private", "Collab Course").await;
    let chapter = create_item(&th.client, TEST_WS, &course, "chapter", "", "Chapter 1").await;
    let unit = create_item(&th.client, TEST_WS, &course, "unit", &chapter.id, "Unit 1").await;
    let unit_path = format!("{TEST_WS}/courses/{course}/units/{}", unit.id);
    write_unit_body(
        &th.client,
        &unit_path,
        unit.updated_at,
        &paragraphs(&["seed"]),
    )
    .await;
    give(&th, &course, &user2, "editor").await;

    let mut one = Peer::connect(&th.client, 501).await;
    let mut two = Peer::connect(&th.client2, 502).await;
    let ch_refused = one.join(TEST_WORKSPACE_ID, &chapter.id).await;
    assert_eq!(
        (&ch_refused["type"], &ch_refused["reason"]),
        (&json!("docRefused"), &json!("not_found")),
        "chapters have no collab room: {ch_refused}"
    );
    one.joined(TEST_WORKSPACE_ID, &unit.id).await;
    two.joined(TEST_WORKSPACE_ID, &unit.id).await;
    one.write("from user one").await;
    two.write("from user two").await;
    for peer in [&mut one, &mut two] {
        peer.text("docAck").await;
        peer.settle().await;
    }
    let merged = one.body().expect("merged body");
    assert_eq!(two.body(), Some(merged.clone()));
    assert_eq!(lines_of(Some(&merged)).len(), 3, "{merged}");
    one.leave().await;
    two.leave().await;
    unit_holds(&th.client2, &unit_path, &merged).await;
    // Automatic body version saving works because BlockType::Unit is in WITH_ROOM.
    let versions = th
        .app
        .store()
        .body_versions(&unit.id)
        .await
        .expect("body versions");
    assert!(!versions.is_empty(), "automatic version saved on unit");

    // (b) Personal space: two separate sessions of the same user co-edit a unit.
    let own = crate::own_space::own_workspace(&th.client).await.id;
    let own_ws = format!("/w/{own}");
    let own_course = create_course(&th.client, &own_ws, "private", "Self Study").await;
    let own_unit = create_item(&th.client, &own_ws, &own_course, "unit", "", "Note 1").await;
    let own_unit_path = format!("{own_ws}/courses/{own_course}/units/{}", own_unit.id);
    write_unit_body(
        &th.client,
        &own_unit_path,
        own_unit.updated_at,
        &paragraphs(&["solo seed"]),
    )
    .await;

    let mut session_b = th.anonymous();
    let logged_in = session_b.login(USER1_USERNAME, PASSWORD).await;
    assert!(logged_in.status.is_success(), "{}", logged_in.text());

    let mut tab_a = Peer::connect(&th.client, 601).await;
    let mut tab_b = Peer::connect(&session_b, 602).await;
    tab_a.joined(&own, &own_unit.id).await;
    tab_b.joined(&own, &own_unit.id).await;
    tab_a.write("from tab a").await;
    tab_b.write("from tab b").await;
    for peer in [&mut tab_a, &mut tab_b] {
        peer.text("docAck").await;
        peer.settle().await;
    }
    let own_merged = tab_a.body().expect("own merged body");
    assert_eq!(tab_b.body(), Some(own_merged.clone()));
    assert_eq!(lines_of(Some(&own_merged)).len(), 3, "{own_merged}");
    tab_a.leave().await;
    tab_b.leave().await;
    unit_holds(&session_b, &own_unit_path, &own_merged).await;

    th.teardown().await;
}

fn archive_of(lines: &[Value]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("c0/container.jsonl", options)
        .expect("start");
    for line in lines {
        writer
            .write_all(format!("{line}\n").as_bytes())
            .expect("write");
    }
    writer.finish().expect("finish").into_inner()
}

fn course_line(id: &str, title: &str) -> Value {
    json!({"type": "container", "data": {
        "id": id, "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "course",
        "title": title, "createdAt": 1, "updatedAt": 1,
    }})
}

fn block_line(
    id: &str,
    course: &str,
    parent: &str,
    block_type: &str,
    body: Option<Value>,
) -> Value {
    let mut data = json!({
        "id": id, "containerId": course, "parentId": parent, "type": block_type,
        "title": id, "createdAt": 1, "updatedAt": 1,
    });
    if let Some(body) = body {
        data["body"] = body;
    }
    json!({"type": "block", "data": data})
}

/// Exporting a course and importing it into another workspace preserves chapters, units,
/// bodies, and files; malformed course archives are refused and store nothing.
#[tokio::test]
async fn course_export_and_import_roundtrip_and_invalid_archives_refused() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;

    let course = create_course(client, TEST_WS, "private", "Algorithms").await;
    let ch = create_item(client, TEST_WS, &course, "chapter", "", "Sorting").await;
    let u1 = create_item(client, TEST_WS, &course, "unit", &ch.id, "Quicksort").await;
    let u2 = create_item(client, TEST_WS, &course, "unit", "", "Overview").await;
    let trashed = create_item(client, TEST_WS, &course, "unit", "", "Dropped").await;
    client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{}", trashed.id))
        .await
        .check_no_content();

    let pic = uploaded_id(&upload(client, &course, "diagram.png", b"png-bytes").await);
    let doc = uploaded_id(&upload(client, &course, "notes.pdf", b"%PDF-notes").await);
    let body = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "partition"}]},
        {"type": "image", "attrs": {"fileId": pic}},
        {"type": "file", "attrs": {"fileId": doc}},
    ]});
    write_unit_body(
        client,
        &format!("{TEST_WS}/courses/{course}/units/{}", u1.id),
        u1.updated_at,
        &body,
    )
    .await;

    let exported = client
        .get(&format!("{TEST_WS}/containers/{course}/archive/export"))
        .await;
    exported.check_ok();

    let me = client.me().await.id;
    let other = another_workspace(&ts, &me).await;
    let at = format!("/w/{other}");
    import(client, &other, &exported.body).await.check_ok();

    let courses: Vec<fvoci_model::Container> = client.get(&format!("{at}/courses")).await.ok_json();
    assert_eq!(courses.len(), 1);
    let imported = &courses[0];
    assert_eq!(
        (imported.kind, imported.title.as_str()),
        (ContainerKind::Course, "Algorithms")
    );
    let tree: Vec<UnitNode> = client
        .get(&format!("{at}/courses/{}/units", imported.id))
        .await
        .ok_json();
    assert_eq!(tree.len(), 3, "trashed unit stays behind: {tree:?}");
    let ch_copy = tree.iter().find(|n| n.title == "Sorting").expect("chapter");
    let u1_copy = tree.iter().find(|n| n.title == "Quicksort").expect("u1");
    let u2_copy = tree.iter().find(|n| n.title == "Overview").expect("u2");
    assert_eq!(
        (ch_copy.kind, ch_copy.parent_id.as_str()),
        (UnitKind::Chapter, "")
    );
    assert_eq!(
        (u1_copy.kind, u1_copy.parent_id.as_str()),
        (UnitKind::Unit, ch_copy.id.as_str())
    );
    assert_eq!(
        (u2_copy.kind, u2_copy.parent_id.as_str()),
        (UnitKind::Unit, "")
    );
    assert_ne!(u1_copy.id, u1.id);
    assert_ne!(u2_copy.id, u2.id);

    let read_u1: Unit = client
        .get(&format!(
            "{at}/courses/{}/units/{}",
            imported.id, u1_copy.id
        ))
        .await
        .ok_json();
    let imported_body = body_of(&read_u1).expect("imported body");
    for (idx, original, expected_bytes) in
        [(1, &pic, &b"png-bytes"[..]), (2, &doc, &b"%PDF-notes"[..])]
    {
        let new_file = imported_body["content"][idx]["attrs"]["fileId"]
            .as_str()
            .expect("copied fileId");
        assert_ne!(new_file, original);
        let bytes = client
            .get(&format!("{at}/containers/{}/files/{new_file}", imported.id))
            .await;
        bytes.check_ok();
        assert_eq!(bytes.body, expected_bytes);
    }
    ts.teardown().await;
}

/// Malformed course archives are refused before storing anything.
#[tokio::test]
async fn invalid_course_archives_are_refused_and_store_nothing() {
    let ts = Setup::native().start().await.init_basic().await;
    let client = &ts.client;
    let me = client.me().await.id;
    let clean_ws = another_workspace(&ts, &me).await;
    let bad_archives: Vec<(&str, Vec<Value>, Option<&str>)> = vec![
        (
            "unit below unit",
            vec![
                course_line("c1", "Bad"),
                block_line("u1", "c1", "", "unit", None),
                block_line("u2", "c1", "u1", "unit", None),
            ],
            None,
        ),
        (
            "chapter below chapter",
            vec![
                course_line("c1", "Bad"),
                block_line("ch1", "c1", "", "chapter", None),
                block_line("ch2", "c1", "ch1", "chapter", None),
            ],
            None,
        ),
        (
            "chapter with body",
            vec![
                course_line("c1", "Bad"),
                block_line("ch1", "c1", "", "chapter", Some(paragraphs(&["illegal"]))),
            ],
            None,
        ),
        (
            "card on course",
            vec![
                course_line("c1", "Bad"),
                block_line("k1", "c1", "", "card", None),
            ],
            None,
        ),
        (
            "too many chapters",
            std::iter::once(course_line("c1", "TooManyCh"))
                .chain(
                    (0..=CHAPTERS_PER_COURSE)
                        .map(|i| block_line(&format!("ch{i}"), "c1", "", "chapter", None)),
                )
                .collect(),
            Some("too_many_chapters"),
        ),
        (
            "too many units",
            std::iter::once(course_line("c1", "TooManyU"))
                .chain(
                    (0..=UNITS_PER_COURSE)
                        .map(|i| block_line(&format!("u{i}"), "c1", "", "unit", None)),
                )
                .collect(),
            Some("too_many_units"),
        ),
    ];
    for (label, lines, expected_reason) in bad_archives {
        let zip = archive_of(&lines);
        let refused = import(client, &clean_ws, &zip).await;
        refused.check_bad_request();
        if let Some(reason) = expected_reason {
            assert_eq!(
                refused.error_reason(),
                reason,
                "{label}: {}",
                refused.text()
            );
        }
    }
    let stored: Vec<Value> = client
        .get(&format!("/w/{clean_ws}/courses"))
        .await
        .ok_json();
    assert!(
        stored.is_empty(),
        "refused archives stored nothing: {stored:?}"
    );

    ts.teardown().await;
}

/// Deleting a chapter flushes unwritten edits of its child units into trash and evicts
/// their rooms; privatizing an open course evicts unjoined workers from open unit rooms
/// and hides it from search, while open-course block changes broadcast to unjoined workers.
#[tokio::test]
async fn deleting_chapter_and_privatizing_open_course_evict_unit_rooms() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;

    // 1) Deleting a chapter while a writer edits a child unit flushes edits and sends docLeft.
    let course = create_course(&th.client, TEST_WS, "private", "Flush Course").await;
    let ch = create_item(&th.client, TEST_WS, &course, "chapter", "", "Ch 1").await;
    let u = create_item(&th.client, TEST_WS, &course, "unit", &ch.id, "U 1").await;
    let u_path = format!("{TEST_WS}/courses/{course}/units/{}", u.id);
    write_unit_body(&th.client, &u_path, u.updated_at, &paragraphs(&["seed"])).await;
    let mut writer = Peer::connect(&th.client, 701).await;
    writer.joined(TEST_WORKSPACE_ID, &u.id).await;
    writer.write_acked("unflushed edit").await;
    let wanted = writer.body();
    th.client
        .delete(&format!("{TEST_WS}/courses/{course}/units/{}", ch.id))
        .await
        .check_no_content();
    let left = writer.text("docLeft").await;
    assert_eq!(left["reason"], json!("not_found"), "{left}");
    let trashed = th
        .app
        .store()
        .get_deleted_block(&u.id)
        .await
        .expect("unit in trash");
    assert_eq!(
        trashed.block.body.map(|b| b.document().clone()),
        wanted,
        "unwritten room edits flushed before chapter deletion"
    );

    // 2) Open course broadcasts blockChanged to unjoined worker, and privatizing evicts room.
    let open = create_course(&th.client, TEST_WS, "open", "Open Room Course").await;
    let open_u = create_item(&th.client, TEST_WS, &open, "unit", "", "OpenUnitToken").await;
    let mut listener =
        WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &user2).await;
    let mut reader = Peer::connect(&th.client2, 702).await;
    let joined = reader.joined(TEST_WORKSPACE_ID, &open_u.id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    th.client
        .patch(
            &format!("{TEST_WS}/courses/{open}/units/{}", open_u.id),
            &json!({"title": "OpenUnitToken updated"}),
        )
        .await
        .check_ok();
    let told = listener
        .expect("blockChanged", "/block/id", &open_u.id)
        .await;
    assert_eq!(told["block"]["type"], json!("unit"), "{told}");
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{open}"),
            &json!({"visibility": "private"}),
        )
        .await
        .check_ok();
    let evicted = reader.text("docLeft").await;
    assert_eq!(evicted["reason"], json!("not_found"), "{evicted}");
    let hidden: Value = th
        .client2
        .get(&format!("{TEST_WS}/search?q=OpenUnitToken"))
        .await
        .ok_json();
    assert_eq!(hidden, json!([]), "privatized course hidden from search");

    th.teardown().await;
}
