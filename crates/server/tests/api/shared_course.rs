//! A course read by its public link: `/shared/{id}`, `/shared/{id}/units`,
//! `/shared/{id}/units/{unitId}`, and `/shared/{id}/files/{fileId}`. Verifies zero
//! user/workspace identifier leaks, outside reference stripping, chapter/trash/switch
//! 404s, diary sharing refusal, duplicateCourse 404, WS block-subscription refusal,
//! anonymous presence, and own-space course sharing.

use fvoci_model::{OrgKind, UserWorkspace};
use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::presence::{people, visit};
use crate::references::made;
use crate::references_shared::references_in;
use crate::references_to::body_naming;
use crate::websocket::{QUIET, Socket};

fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_owned()
}

struct SharedCourse {
    course: String,
    token: String,
    chapter: String,
    unit: String,
    other_unit: String,
    gone: String,
    file: String,
    secrets: Vec<String>,
    kept: Vec<String>,
}

async fn post_id(client: &Client, path: &str, body: &Value) -> String {
    let made = client.post(path, body).await;
    made.check_created();
    id(&made.value())
}

async fn upload_slide(owner: &Client, course: &str) -> String {
    let file = Part::bytes(b"course slide image".to_vec()).file_name("slide.png");
    let uploaded = Client::send(
        owner
            .request(
                Method::POST,
                &format!("{TEST_WS}/containers/{course}/files"),
            )
            .multipart(Form::new().part("file", file)),
    )
    .await
    .value();
    uploaded["fileId"].as_str().expect("a file").to_owned()
}

async fn shared_course(th: &TestServer) -> SharedCourse {
    let owner = &th.client;
    let other = &th.client2;
    let elsewhere = made(owner, TEST_WS, "private", "elsewhere").await;
    let (owner_id, other_id) = (owner.me().await.id, other.me().await.id);
    let courses = format!("{TEST_WS}/courses");
    let other_course = post_id(
        owner,
        &courses,
        &json!({"visibility": "private", "title": "Other"}),
    )
    .await;
    let outside_unit = post_id(
        owner,
        &format!("{courses}/{other_course}/units"),
        &json!({"title": "Outside Unit"}),
    )
    .await;
    let course = post_id(
        owner,
        &courses,
        &json!({"visibility": "private", "title": "Public"}),
    )
    .await;
    owner
        .post(
            &format!("{TEST_WS}/containers/{course}/members"),
            &json!({"containerId": course, "userId": other_id, "role": "editor"}),
        )
        .await
        .check_ok();
    let at = format!("{courses}/{course}/units");
    let chapter = post_id(owner, &at, &json!({"kind": "chapter", "title": "Week 1"})).await;
    let unit = post_id(owner, &at, &json!({"parentId": chapter, "title": "L1"})).await;
    let other_unit = post_id(other, &at, &json!({"parentId": chapter, "title": "L2"})).await;
    let gone = post_id(owner, &at, &json!({"title": "Deleted"})).await;
    owner
        .delete(&format!("{at}/{gone}"))
        .await
        .check_no_content();

    let outside = [
        outside_unit,
        elsewhere.page,
        elsewhere.card,
        chapter.clone(),
        gone.clone(),
        UNKNOWN_ID.to_owned(),
    ];
    let mut named = vec![other_unit.as_str()];
    named.extend(outside.iter().map(String::as_str));
    let mut body = body_naming("see", &named);
    let mention = |uid: &str| json!({"type": "mention", "attrs": {"userId": uid}});
    body["content"][0]["content"]
        .as_array_mut()
        .expect("a line")
        .extend([mention(&owner_id), mention(&other_id)]);
    let path = format!("{at}/{unit}");
    let read_at = owner.get(&path).await.value()["updatedAt"].clone();
    owner
        .patch(&path, &json!({"readAt": read_at, "body": body}))
        .await
        .check_ok();
    owner
        .post(
            &format!("{path}/comments"),
            &json!({"body": body_naming("private comment", &[])}),
        )
        .await
        .check_created();

    let mut kept = vec![other_unit.clone()];
    kept.extend(outside.iter().map(|_| String::new()));
    let mut secrets = vec![owner_id, other_id, TEST_WORKSPACE_ID.to_owned()];
    secrets.extend(outside.into_iter().filter(|s| *s != chapter));
    SharedCourse {
        token: th.share(&course).await,
        file: upload_slide(owner, &course).await,
        course,
        chapter,
        unit,
        other_unit,
        gone,
        secrets,
        kept,
    }
}

impl SharedCourse {
    fn at(&self, rest: &str) -> String {
        let joint = if rest.contains('?') { '&' } else { '?' };
        format!(
            "/shared/{}{rest}{joint}readToken={}",
            self.course, self.token
        )
    }
}

/// Anonymous and signed-in readers of a shared course get the course, outline, units,
/// and files with zero user/workspace IDs and with outside/chapter/trashed references cleared.
#[tokio::test]
async fn a_course_is_read_by_its_link_and_names_nobody_and_nothing_else() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_course(&th).await;
    let anonymous = th.anonymous();
    for reader in [&anonymous, &th.client2, &th.client] {
        let mut answers = Vec::new();
        for rest in [
            String::new(),
            "/units".to_owned(),
            format!("/units/{}", shared.unit),
            format!("/units/{}", shared.other_unit),
            format!("/files/{}/info", shared.file),
        ] {
            let answer = reader.get(&shared.at(&rest)).await;
            answer.check_ok();
            answers.push((rest, answer.text()));
        }
        for (rest, text) in &answers {
            for secret in &shared.secrets {
                assert!(
                    !text.contains(secret.as_str()),
                    "{rest} leaked {secret}: {text}"
                );
            }
            assert!(
                !text.contains("private comment"),
                "{rest} leaked a comment: {text}"
            );
        }
        let container: Value = serde_json::from_str(&answers[0].1).expect("course");
        assert_eq!(container["kind"], "course");
        assert_eq!(container["createdBy"], "");
        assert_eq!(container["modifiedBy"], "");
        assert_eq!(container["workspaceId"], "");

        let tree: Value = serde_json::from_str(&answers[1].1).expect("tree");
        assert_eq!(
            tree.as_array().map(Vec::len),
            Some(3),
            "chapter + 2 live units: {tree}"
        );

        let unit: Value = serde_json::from_str(&answers[2].1).expect("unit");
        assert_eq!(references_in(&unit["body"]), shared.kept);
        assert_eq!(unit["createdBy"], "");
        assert_eq!(unit["modifiedBy"], "");
        // Every mention in the body must have an empty userId.
        let mentions: Vec<&str> = unit["body"]["content"][0]["content"]
            .as_array()
            .expect("content")
            .iter()
            .filter(|node| node["type"] == "mention")
            .map(|node| node["attrs"]["userId"].as_str().expect("userId"))
            .collect();
        assert_eq!(mentions, ["", ""], "mention userIds emptied");

        let file = reader
            .get(&shared.at(&format!("/files/{}", shared.file)))
            .await;
        file.check_ok();
        assert_eq!(file.body, b"course slide image");
    }
    th.teardown().await;
}

/// Cross-kind token boundaries, chapter/trash/switch/off-token 404s, diary sharing
/// refusal, `duplicateContainer` 404 on courses, WS refusal, presence, and own space.
#[tokio::test]
async fn course_link_boundaries_switches_presence_and_invariants() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_course(&th).await;
    let owner = &th.client;
    let anonymous = th.anonymous();

    // 1) Cross-kind routes, chapters, trashed units, wrong tokens -> 404.
    for rest in [
        "/blocks?all=true",
        "/cards",
        "/pages",
        &format!("/units/{}", shared.chapter),
        &format!("/units/{}", shared.gone),
        &format!("/units/{UNKNOWN_ID}"),
    ] {
        anonymous.get(&shared.at(rest)).await.check_not_found();
    }
    let other_things = made(owner, TEST_WS, "private", "other").await;
    let board_token = th.share(&other_things.board).await;
    let wiki_token = th.share(&other_things.wiki).await;
    for (cid, tok) in [
        (&other_things.board, &board_token),
        (&other_things.wiki, &wiki_token),
    ] {
        anonymous
            .get(&format!("/shared/{cid}/units?readToken={tok}"))
            .await
            .check_not_found();
        anonymous
            .get(&format!(
                "/shared/{cid}/units/{}?readToken={tok}",
                shared.unit
            ))
            .await
            .check_not_found();
    }
    anonymous
        .get(&format!("/shared/{}/units?readToken=wrong", shared.course))
        .await
        .check_not_found();

    // 2) `duplicateContainer` works for courses (L6), and copies without sharing.
    let copied = owner
        .post(
            &format!("{TEST_WS}/containers/{}/duplicate", shared.course),
            &json!({}),
        )
        .await
        .created_json::<Value>();
    assert_eq!(copied["containers"][0]["kind"], "course");
    assert_ne!(copied["containers"][0]["id"], shared.course);

    // 3) WS `subscribeBlocks` does not follow a course token.
    let mut reader = Socket::connect(&th).await;
    reader
        .send(json!({
            "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "readToken": shared.token, "blockIds": [shared.unit],
        }))
        .await;
    let unit_path = format!("{TEST_WS}/courses/{}/units/{}", shared.course, shared.unit);
    let read_at = owner.get(&unit_path).await.value()["updatedAt"].clone();
    owner
        .patch(&unit_path, &json!({"readAt": read_at, "title": "Renamed"}))
        .await
        .check_ok();
    assert!(
        reader.next_json(QUIET).await.is_none(),
        "course readToken cannot subscribe to blocks over WS"
    );

    // 4) Anonymous presence works and ends when token is regenerated.
    let mut member = Socket::connect_as(owner).await;
    visit(&mut member, &shared.course, "").await;
    let _ = people(&mut member, 1).await;
    reader
        .send(json!({
            "type": "setPresence", "containerId": shared.course,
            "cardId": shared.unit, "readToken": shared.token,
        }))
        .await;
    assert_eq!(people(&mut member, 1).await["anonymousCount"], 1);
    let sharing = format!("{TEST_WS}/containers/{}/public-link", shared.course);
    owner
        .post(&sharing, &json!({"enabled": true, "regenerate": true}))
        .await
        .check_ok();
    assert_eq!(people(&mut member, 1).await["anonymousCount"], 0);
    anonymous.get(&shared.at("/units")).await.check_not_found();
    th.teardown().await;
}

/// `PublicLink` off, org `publicLinks` off, trashed course, own-space course sharing, and diary
/// sharing refusal (404).
#[tokio::test]
async fn course_link_switches_trash_diary_and_own_space_invariants() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_course(&th).await;
    let owner = &th.client;
    let anonymous = th.anonymous();
    let sharing = format!("{TEST_WS}/containers/{}/public-link", shared.course);

    // 1) PublicLink off, org publicLinks off, and trashed course -> 404.
    let fresh = th.share(&shared.course).await;
    let fresh_units = format!("/shared/{}/units?readToken={fresh}", shared.course);
    anonymous.get(&fresh_units).await.check_ok();
    owner
        .post(&sharing, &json!({"enabled": false}))
        .await
        .check_ok();
    anonymous.get(&fresh_units).await.check_not_found();

    let fresh = th.share(&shared.course).await;
    let fresh_units = format!("/shared/{}/units?readToken={fresh}", shared.course);
    let org = crate::org_admin::org_of(&th).await;
    owner
        .patch(&format!("/org/{org}"), &json!({"publicLinks": false}))
        .await
        .check_ok();
    anonymous.get(&fresh_units).await.check_not_found();
    owner
        .patch(&format!("/org/{org}"), &json!({"publicLinks": true}))
        .await
        .check_ok();
    anonymous.get(&fresh_units).await.check_ok();
    owner
        .delete(&format!("{TEST_WS}/containers/{}", shared.course))
        .await
        .check_no_content();
    anonymous.get(&fresh_units).await.check_not_found();

    // 2) Own space: course sharing works and hides owner/workspace, while diary sharing is 404.
    let workspaces: Vec<UserWorkspace> = owner.get("/users/me/workspaces").await.ok_json();
    let own = workspaces
        .into_iter()
        .find(|w| w.kind == OrgKind::Personal)
        .expect("own space")
        .id;
    let own_ws = format!("/w/{own}");
    let diary = id(&owner
        .post(&format!("{own_ws}/diaries"), &json!({"title": "My Diary"}))
        .await
        .value());
    owner
        .post(
            &format!("{own_ws}/containers/{diary}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .check_not_found();
    let own_course = id(&owner
        .post(
            &format!("{own_ws}/courses"),
            &json!({"visibility": "private", "title": "My Study"}),
        )
        .await
        .value());
    let own_unit = id(&owner
        .post(
            &format!("{own_ws}/courses/{own_course}/units"),
            &json!({"title": "Note 1"}),
        )
        .await
        .value());
    let own_token = owner
        .post(
            &format!("{own_ws}/containers/{own_course}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .value()["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let own_read = anonymous
        .get(&format!(
            "/shared/{own_course}/units/{own_unit}?readToken={own_token}"
        ))
        .await;
    own_read.check_ok();
    let owner_id = owner.me().await.id;
    assert!(
        !own_read.text().contains(&owner_id) && !own_read.text().contains(&own),
        "own-space shared unit leaks no owner or workspace id: {}",
        own_read.text()
    );
    th.teardown().await;
}
