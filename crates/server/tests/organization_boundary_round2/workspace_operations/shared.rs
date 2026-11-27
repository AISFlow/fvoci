//! Read-token HTTP paths use the token's board or wiki and never reveal people.

use reqwest::multipart::{Form, Part};
use serde_json::json;

use super::Case;

pub(super) async fn exercise(case: &mut Case) {
    let made = case
        .run(
            "createBoard",
            &[],
            json!({"title": format!("{} shared", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let board = case.remember(&made, "id");
    let at = &[("boardId", board.as_str())];
    let made = case
        .run(
            "createCard",
            at,
            json!({"title": format!("{} shared card", case.label)}),
        )
        .await
        .value();
    let card = case.remember(&made, "id");
    let sharing = case
        .run("setPublicLink", at, json!({"enabled": true}))
        .await
        .value();
    let token = sharing["token"].as_str().expect("read token");
    for (operation, expected) in [
        ("getSharedContainer", &board),
        ("getSharedBlocks", &card),
        ("getSharedCards", &card),
    ] {
        let request = super::with_query(case.request(operation, at), "readToken", token);
        let response = case.send(operation, request).await;
        assert!(
            response.text().contains(expected),
            "{operation} returns its own content"
        );
        assert!(
            !response.text().contains(&case.user),
            "{operation} hides user IDs even with a session"
        );
        assert!(
            !response.text().contains(&case.workspace),
            "{operation} hides the workspace"
        );
    }
    let payload = format!("{} shared file", case.label).into_bytes();
    let name = format!("{}-shared.txt", case.label);
    let request = case
        .request("uploadFile", at)
        .multipart(Form::new().part("file", Part::bytes(payload.clone()).file_name(name.clone())));
    let uploaded = case.send("uploadFile", request).await.value();
    let file = case.remember(&uploaded, "fileId");
    let at = &[("boardId", board.as_str()), ("fileId", file.as_str())];
    let request = super::with_query(case.request("getSharedFile", at), "readToken", token);
    let read = case.send("getSharedFile", request).await;
    assert_eq!(
        read.body, payload,
        "public file belongs to this organization"
    );
    let request = super::with_query(case.request("getSharedFileRecord", at), "readToken", token);
    let info = case.send("getSharedFileRecord", request).await.value();
    assert_eq!(info["name"], name, "public file metadata names this upload");
}

/// A wiki's link reads its tree and its pages, of this organization, naming nobody.
pub(super) async fn exercise_wiki(case: &mut Case) {
    let made = case
        .run(
            "createWiki",
            &[],
            json!({"title": format!("{} shared wiki", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let wiki = case.remember(&made, "id");
    let made = case
        .run(
            "createPage",
            &[("wikiId", wiki.as_str())],
            json!({"title": format!("{} shared page", case.label)}),
        )
        .await
        .value();
    let page = case.remember(&made, "id");
    let at = &[("containerId", wiki.as_str())];
    let sharing = case
        .run("setPublicLink", at, json!({"enabled": true}))
        .await
        .value();
    let token = sharing["token"].as_str().expect("read token");
    let of_page = &[("containerId", wiki.as_str()), ("pageId", page.as_str())];
    for (operation, params, expected) in [
        ("getSharedPageTree", &at[..], &page),
        ("getSharedPage", &of_page[..], &page),
    ] {
        let request = super::with_query(case.request(operation, params), "readToken", token);
        let response = case.send(operation, request).await;
        assert!(
            response.text().contains(expected.as_str()),
            "{operation} returns its own content"
        );
        assert!(
            !response.text().contains(&case.user),
            "{operation} hides user IDs even with a session"
        );
        assert!(
            !response.text().contains(&case.workspace),
            "{operation} hides the workspace"
        );
    }
}

/// A course's link reads its outline and its units, of this organization, naming nobody.
pub(super) async fn exercise_course(case: &mut Case) {
    let made = case
        .run(
            "createCourse",
            &[],
            json!({"title": format!("{} shared course", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let course = case.remember(&made, "id");
    let made = case
        .run(
            "createUnit",
            &[("courseId", course.as_str())],
            json!({"title": format!("{} shared unit", case.label)}),
        )
        .await
        .value();
    let unit = case.remember(&made, "id");
    let at = &[("containerId", course.as_str())];
    let sharing = case
        .run("setPublicLink", at, json!({"enabled": true}))
        .await
        .value();
    let token = sharing["token"].as_str().expect("read token");
    let of_unit = &[("containerId", course.as_str()), ("unitId", unit.as_str())];
    for (operation, params, expected) in [
        ("getSharedUnitTree", &at[..], &unit),
        ("getSharedUnit", &of_unit[..], &unit),
    ] {
        let request = super::with_query(case.request(operation, params), "readToken", token);
        let response = case.send(operation, request).await;
        assert!(
            response.text().contains(expected.as_str()),
            "{operation} returns its own content"
        );
        assert!(
            !response.text().contains(&case.user),
            "{operation} hides user IDs even with a session"
        );
        assert!(
            !response.text().contains(&case.workspace),
            "{operation} hides the workspace"
        );
    }
}
