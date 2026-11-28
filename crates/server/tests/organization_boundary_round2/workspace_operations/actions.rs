//! Successful operations on real objects, with destructive cases after their reads.

use serde_json::json;

use super::{Case, files};

/// 👍 as a path gives it.
const THUMBS_UP: &str = "%F0%9F%91%8D";

pub(super) async fn exercise(case: &mut Case) {
    let workspace = case.workspace.clone();
    case.present("getWorkspace", &[], &workspace).await;
    case.run("getAccountToday", &[], json!({})).await;
    case.run("getWorkspaceToday", &[], json!({})).await;
    let made = case
        .run(
            "createBoard",
            &[],
            json!({"title": format!("secret {} board", case.label), "visibility": "open", "cardProperties": [{"id": "first", "name": "First", "type": "text"}, {"id": "second", "name": "Second", "type": "text"}]}),
        )
        .await
        .value();
    let board = case.remember(&made, "id");
    boards(case, &board).await;
    wikis(case).await;
    courses(case).await;
    let card = cards(case, &board).await;
    case.run(
        "appendCardMarkdown",
        &[("cardId", &card)],
        json!({"markdown": format!("{} added", case.label)}),
    )
    .await;
    categories(case, &board).await;
    stars(case, &board).await;
    let audited = people(case, &board).await;
    subscriptions(case, &card).await;
    files::exercise(case, &board, &card).await;
    templates(case).await;
    case.run("deleteContainer", &[("boardId", &board)], json!({}))
        .await;
    let trash = case.run("getDeletedContainers", &[], json!({})).await;
    assert!(
        trash.text().contains(&board),
        "the deleted board is in this founder's trash"
    );
    case.run("restoreContainer", &[("boardId", &board)], json!({}))
        .await;
    // Keep private fixtures behind for the next founder's template and trash reads.
    for template in [true, false] {
        let made = case
            .run(
                "createBoard",
                &[],
                json!({"title": format!("{} private fixture", case.label), "visibility": "private", "isTemplate": template}),
            )
            .await
            .value();
        let fixture = case.remember(&made, "id");
        if !template {
            case.run("deleteContainer", &[("boardId", &fixture)], json!({}))
                .await;
        }
    }
    case.present("getWorkspaceAudit", &[], &audited).await;
}

async fn boards(case: &mut Case, board: &str) {
    let at = &[("boardId", board)];
    case.present("getWorkspaceBoards", &[], board).await;
    case.present("listWorkspaceContainers", &[], board).await;
    case.present("getWorkspaceOverview", &[], board).await;
    case.present("getContainer", at, board).await;
    case.run("getMyContainerPermissions", at, json!({})).await;
    let ws = case.workspace.clone();
    let copied_ws = case
        .run(
            "copyContainerToWorkspace",
            at,
            json!({"targetWorkspaceId": ws, "title": format!("secret {} ws copy", case.label)}),
        )
        .await
        .value();
    let ws_copy = case.remember(&copied_ws["container"], "id");
    case.run("deleteContainer", &[("boardId", &ws_copy)], json!({}))
        .await;
    case.run(
        "patchContainer",
        at,
        json!({"title": format!("secret {} renamed", case.label)}),
    )
    .await;
    let found = case.run("searchWorkspaceBoards", &[], json!({})).await;
    assert!(
        found.text().contains(board),
        "search finds the founder's board"
    );
    let order = case
        .run("reorderCardProperties", at, json!(["second", "first"]))
        .await
        .value();
    assert_eq!(
        order,
        json!(["second", "first"]),
        "nonempty properties are reordered"
    );
    case.run("setPublicLink", at, json!({"enabled": true}))
        .await;
    let sharing = case.run("getPublicLink", at, json!({})).await.value();
    assert_eq!(sharing["enabled"], true, "sharing was enabled");
}

/// A wiki of the founder's, listed among the wikis and read as a board is.
async fn wikis(case: &mut Case) {
    let made = case
        .run(
            "createWiki",
            &[],
            json!({"title": format!("secret {} wiki", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let wiki = case.remember(&made, "id");
    assert_eq!(made["kind"], "wiki", "a wiki is made as one");
    case.present("getWikis", &[], &wiki).await;
    case.present("getContainer", &[("containerId", &wiki)], &wiki)
        .await;
    pages(case, &wiki).await;
}

/// A course of the founder's with a chapter and a unit: made, listed, read, changed,
/// moved, deleted, found in the trash and brought back.
async fn courses(case: &mut Case) {
    let made = case
        .run(
            "createCourse",
            &[],
            json!({"title": format!("secret {} course", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let course = case.remember(&made, "id");
    assert_eq!(made["kind"], "course", "a course is made as one");
    case.present("getCourses", &[], &course).await;
    let at = &[("courseId", course.as_str())];
    let chapter = case
        .run(
            "createUnit",
            at,
            json!({"kind": "chapter", "title": format!("secret {} chapter", case.label)}),
        )
        .await
        .value();
    let chapter = case.remember(&chapter, "id");
    let unit = case
        .run(
            "createUnit",
            at,
            json!({"parentId": chapter, "title": format!("secret {} unit", case.label)}),
        )
        .await
        .value();
    let unit = case.remember(&unit, "id");
    case.present("getUnitTree", at, &unit).await;
    let of_unit = &[("courseId", course.as_str()), ("unitId", unit.as_str())];
    case.present("getUnit", of_unit, &unit).await;
    let told = case
        .run("resolveReferences", &[], json!([unit]))
        .await
        .value();
    assert_eq!(told[0]["id"], unit, "unit reference resolves");
    case.present("searchReferences", &[], &unit).await;
    case.run("getReferencesTo", &[("blockId", unit.as_str())], json!({}))
        .await;
    case.run("patchUnit", of_unit, json!({"title": "changed unit"}))
        .await;
    unit_comments(case, &course, &unit).await;
    unit_versions(case, &course, &unit).await;
    let user = case.user.clone();
    case.run(
        "putUnitProgress",
        of_unit,
        json!({"completed": true, "lastPosition": format!("secret {} progress", case.label)}),
    )
    .await;
    case.present("getMyCourseProgress", at, &unit).await;
    case.present("getCourseProgressSummary", at, &user).await;
    course_events(case, &course, &unit, &user).await;
    unit_reviews_and_notes(case, &course, &unit).await;
    case.run(
        "putCourseSchedule",
        at,
        json!({"startDate": "2026-11-01", "targetDate": "2026-11-30", "studyDays": 127, "autoAssign": true}),
    )
    .await;
    case.present("getCourseSchedule", at, &unit).await;
    case.run(
        "redistributeCourseSchedule",
        at,
        json!({"fromDate": "2026-11-02", "targetDate": "2026-11-30", "studyDays": 127}),
    )
    .await;
    case.run(
        "putUnitSchedule",
        of_unit,
        json!({"plannedDate": "2026-11-24"}),
    )
    .await;
    case.run(
        "redistributeMyCourseSchedule",
        at,
        json!({"fromDate": "2026-11-03", "targetDate": "2026-11-30", "studyDays": 127}),
    )
    .await;
    case.run("getWorkspaceLearningToday", &[], json!({})).await;
    case.run("deleteCourseSchedule", at, json!({})).await;
    case.run("moveUnit", of_unit, json!({"parentId": ""})).await;
    case.run("deleteUnit", of_unit, json!({})).await;
    case.present("getDeletedUnits", at, &unit).await;
    case.run("restoreUnit", of_unit, json!({})).await;
}

/// Course study meetings, attendance, exams, and personal exam scores.
async fn course_events(case: &mut Case, course: &str, unit: &str, user: &str) {
    let at = &[("courseId", course)];
    let meeting = case
        .run(
            "createCourseMeeting",
            at,
            json!({
                "title": format!("secret {} meeting", case.label),
                "startsAt": 1_800_000_000_000_i64,
                "location": "room 1",
                "unitIds": [unit],
                "presenterId": user,
                "hostId": user,
                "notes": format!("secret {} meeting notes", case.label),
            }),
        )
        .await
        .value();
    let meeting_id = case.remember(&meeting, "id");
    case.present("listCourseMeetings", at, &meeting_id).await;
    let of_meeting = &[("courseId", course), ("meetingId", meeting_id.as_str())];
    case.run(
        "patchCourseMeeting",
        of_meeting,
        json!({"title": format!("secret {} meeting updated", case.label)}),
    )
    .await;
    case.run(
        "putMeetingAttendance",
        &[
            ("courseId", course),
            ("meetingId", meeting_id.as_str()),
            ("userId", user),
        ],
        json!({"status": "present"}),
    )
    .await;
    case.run("deleteCourseMeeting", of_meeting, json!({})).await;

    let exam = case
        .run(
            "createCourseExam",
            at,
            json!({
                "title": format!("secret {} exam", case.label),
                "examDate": "2026-12-01",
                "maxScore": 100.0,
                "description": format!("secret {} exam desc", case.label),
            }),
        )
        .await
        .value();
    let exam_id = case.remember(&exam, "id");
    let of_exam = &[("courseId", course), ("examId", exam_id.as_str())];
    case.run(
        "patchCourseExam",
        of_exam,
        json!({"title": format!("secret {} exam updated", case.label)}),
    )
    .await;
    case.run(
        "putMyExamScore",
        of_exam,
        json!({"score": 95.0, "note": format!("secret {} exam note", case.label)}),
    )
    .await;
    case.present("listCourseExams", at, &exam_id).await;
    case.run("deleteMyExamScore", of_exam, json!({})).await;
    case.run("deleteCourseExam", of_exam, json!({})).await;
}

/// Personal review schedules and study notes on a unit.
async fn unit_reviews_and_notes(case: &mut Case, course: &str, unit: &str) {
    let at = &[("courseId", course)];
    let of_unit = &[("courseId", course), ("unitId", unit)];
    let review = case
        .run(
            "scheduleUnitReview",
            of_unit,
            json!({"scheduledDate": "2020-01-01", "intervalDays": 1}),
        )
        .await
        .value()["id"]
        .as_str()
        .expect("a review")
        .to_owned();
    case.present("listMyCourseReviews", at, &review).await;
    case.present("getWorkspaceTodayReviews", &[], &review).await;
    let of_review = &[("courseId", course), ("reviewId", review.as_str())];
    case.run("completeUnitReview", of_review, json!({})).await;
    case.run("deleteUnitReview", of_review, json!({})).await;
    let note = case
        .run(
            "createUnitNote",
            of_unit,
            json!({"kind": "question", "content": format!("secret {} note", case.label)}),
        )
        .await
        .value()["id"]
        .as_str()
        .expect("a note")
        .to_owned();
    case.present("listMyUnitNotes", of_unit, &note).await;
    let of_note = &[("courseId", course), ("noteId", note.as_str())];
    case.run(
        "patchUnitNote",
        of_note,
        json!({"kind": "review", "content": format!("secret {} updated note", case.label)}),
    )
    .await;
    case.run("deleteUnitNote", of_note, json!({})).await;
}

/// The versions of the unit's body: two written, listed, the older read and gone back
/// to, and kept by hand.
async fn unit_versions(case: &mut Case, course: &str, unit: &str) {
    let of_unit = &[("courseId", course), ("unitId", unit)];
    for said in ["first", "second"] {
        let read = case.present("getUnit", of_unit, unit).await.value();
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": format!("secret {} {said} unit body", case.label)}]}]});
        case.run(
            "patchUnit",
            of_unit,
            json!({"readAt": read["updatedAt"], "body": body}),
        )
        .await;
    }
    let listed = case
        .run("getUnitBodyVersions", of_unit, json!({}))
        .await
        .value();
    let version = listed[1]["version"]
        .as_i64()
        .expect("an older unit version")
        .to_string();
    let of_version = &[
        ("courseId", course),
        ("unitId", unit),
        ("version", version.as_str()),
    ];
    case.run("getUnitBodyVersion", of_version, json!({})).await;
    case.run("restoreUnitBodyVersion", of_version, json!({}))
        .await;
    case.run("keepUnitBodyVersion", of_unit, json!({})).await;
}

/// A comment on the unit: written, listed, changed, its versions read, deleted and
/// brought back.
async fn unit_comments(case: &mut Case, course: &str, unit: &str) {
    let of_unit = &[("courseId", course), ("unitId", unit)];
    let said = |text: String| {
        json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": text}]}]})
    };
    let body = said(format!("secret {} unit comment", case.label));
    let made = case
        .run("createUnitComment", of_unit, json!({ "body": body }))
        .await
        .value();
    let comment = case.remember(&made, "id");
    case.present("getUnitComments", of_unit, &comment).await;
    let of_comment = &[
        ("courseId", course),
        ("unitId", unit),
        ("commentId", comment.as_str()),
    ];
    let change =
        json!({"body": said("changed unit comment".to_owned()), "readAt": made["updatedAt"]});
    case.run("patchUnitComment", of_comment, change).await;
    case.run("getUnitCommentVersions", of_comment, json!({}))
        .await;
    case.run("deleteUnitComment", of_comment, json!({})).await;
    case.run("restoreUnitComment", of_comment, json!({})).await;
}

/// Pages of the wiki, one below the other: made, listed, read, changed, moved,
/// deleted, found in the trash and brought back.
async fn pages(case: &mut Case, wiki: &str) {
    let at = &[("wikiId", wiki)];
    let title = format!("secret {} page", case.label);
    let made = case
        .run("createPage", at, json!({"title": title}))
        .await
        .value();
    let page = case.remember(&made, "id");
    let below = case
        .run(
            "createPage",
            at,
            json!({"title": "below", "parentId": page}),
        )
        .await
        .value();
    let below = case.remember(&below, "id");
    case.present("getPageTree", at, &page).await;
    let of_page = &[("wikiId", wiki), ("pageId", page.as_str())];
    case.present("getPage", of_page, &page).await;
    // What a body names is told of this organization's page, and of nothing that the
    // other organization made.
    let mut named = vec![page.clone()];
    named.extend(
        case.forbidden
            .iter()
            .filter(|theirs| fvoci_model::is_id(theirs))
            .take(50)
            .cloned(),
    );
    let told = case
        .run("resolveReferences", &[], json!(named))
        .await
        .value();
    let told: Vec<&str> = told
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|linked| linked["id"].as_str())
        .collect();
    assert_eq!(told, [page.as_str()], "what the references name");
    case.present("searchReferences", &[], &page).await;
    case.run("getReferencesTo", &[("blockId", page.as_str())], json!({}))
        .await;
    case.run("patchPage", of_page, json!({"title": "changed page"}))
        .await;
    page_comments(case, wiki, &page).await;
    page_versions(case, wiki, &page).await;
    let of_below = &[("wikiId", wiki), ("pageId", below.as_str())];
    case.run("movePage", of_below, json!({"parentId": ""}))
        .await;
    case.run("duplicatePage", of_below, json!({"title": "a copy"}))
        .await;
    case.run("deletePage", of_below, json!({})).await;
    case.present("getDeletedPages", at, &below).await;
    case.run("restorePage", of_below, json!({})).await;
    case.run("deletePage", of_below, json!({})).await;
}

/// The versions of the page's body: two written, listed, the older read and gone back
/// to, which writes it to the page again.
async fn page_versions(case: &mut Case, wiki: &str, page: &str) {
    let of_page = &[("wikiId", wiki), ("pageId", page)];
    for said in ["first", "second"] {
        let read = case.present("getPage", of_page, page).await.value();
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": format!("secret {} {said} body", case.label)}]}]});
        case.run(
            "patchPage",
            of_page,
            json!({"readAt": read["updatedAt"], "body": body}),
        )
        .await;
    }
    let listed = case
        .run("getPageBodyVersions", of_page, json!({}))
        .await
        .value();
    let version = listed[1]["version"]
        .as_i64()
        .expect("an older version")
        .to_string();
    let of_version = &[
        ("wikiId", wiki),
        ("pageId", page),
        ("version", version.as_str()),
    ];
    case.run("getPageBodyVersion", of_version, json!({})).await;
    case.run("restorePageBodyVersion", of_version, json!({}))
        .await;
    case.run("keepPageBodyVersion", of_page, json!({})).await;
}

/// A comment on the page: written, listed, changed, its versions read, deleted and
/// brought back.
async fn page_comments(case: &mut Case, wiki: &str, page: &str) {
    let of_page = &[("wikiId", wiki), ("pageId", page)];
    let said = |text: String| {
        json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": text}]}]})
    };
    let body = said(format!("secret {} comment", case.label));
    let made = case
        .run("createPageComment", of_page, json!({ "body": body }))
        .await
        .value();
    let comment = case.remember(&made, "id");
    case.present("getPageComments", of_page, &comment).await;
    let of_comment = &[
        ("wikiId", wiki),
        ("pageId", page),
        ("commentId", comment.as_str()),
    ];
    let change = json!({"body": said("changed comment".to_owned()), "readAt": made["updatedAt"]});
    case.run("patchPageComment", of_comment, change).await;
    case.run("getPageCommentVersions", of_comment, json!({}))
        .await;
    case.run("deletePageComment", of_comment, json!({})).await;
    case.run("restorePageComment", of_comment, json!({})).await;
}

async fn cards(case: &mut Case, board: &str) -> String {
    let at = &[("boardId", board)];
    let made = case
        .run(
            "createCard",
            at,
            json!({"title": "parity card", "properties": {}}),
        )
        .await
        .value();
    let card = case.remember(&made, "id");
    case.present("getCards", at, &card).await;
    case.present("getCard", &[("cardId", &card)], &card).await;
    let number = made["number"].to_string();
    let by_number = &[("boardId", board), ("number", number.as_str())];
    case.present("getCardByNumber", by_number, &card).await;
    case.run(
        "patchCard",
        &[("cardId", &card)],
        json!({"title": "changed card"}),
    )
    .await;
    let read = case
        .run("getCard", &[("cardId", &card)], json!({}))
        .await
        .value();
    case.run(
        "patchCard",
        &[("cardId", &card)],
        json!({"readAt": read["updatedAt"], "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "a kept body"}]}]}}),
    )
    .await;
    case.present("getCardBodyVersions", &[("cardId", &card)], "\"version\":1")
        .await;
    case.present(
        "getCardActivity",
        &[("cardId", &card)],
        "\"kind\":\"created\"",
    )
    .await;
    case.present(
        "getCardBodyVersion",
        &[("cardId", &card), ("version", "1")],
        "a kept body",
    )
    .await;
    // Another body, so that the restore writes the card's.
    let read = case
        .run("getCard", &[("cardId", &card)], json!({}))
        .await
        .value();
    case.run(
        "patchCard",
        &[("cardId", &card)],
        json!({"readAt": read["updatedAt"], "body": null}),
    )
    .await;
    case.run(
        "restoreCardBodyVersion",
        &[("cardId", &card), ("version", "1")],
        json!({}),
    )
    .await;
    case.run("keepCardBodyVersion", &[("cardId", &card)], json!({}))
        .await;
    case.run(
        "batchPatchCards",
        at,
        json!({"cardIds": [card], "updatedProperties": {}}),
    )
    .await;
    case.run("batchDeleteCards", at, json!({"cardIds": [card]}))
        .await;
    let trash = case.run("getDeletedCards", at, json!({})).await;
    assert!(
        trash.text().contains(&card),
        "deleted card is visible in its trash"
    );
    case.run("batchRestoreCards", at, json!({"cardIds": [card]}))
        .await;
    board_views_and_card_ops(case, board, &card).await;
    card_comments(case, board, &card).await;
    card
}

async fn board_views_and_card_ops(case: &mut Case, board: &str, card: &str) {
    let at = &[("boardId", board)];
    let created_view = case
        .run(
            "createBoardView",
            at,
            json!({"title": format!("secret {} view", case.label), "viewType": "board"}),
        )
        .await
        .value();
    let view = case.remember(&created_view["view"], "id");
    let of_view = &[("boardId", board), ("viewId", view.as_str())];
    case.present("getBoardViews", at, &view).await;
    case.present("getBoardView", of_view, &view).await;
    case.run(
        "patchBoardView",
        of_view,
        json!({"title": format!("secret {} view renamed", case.label)}),
    )
    .await;
    let dup_view = case
        .run(
            "duplicateBoardView",
            of_view,
            json!({"title": format!("secret {} view copy", case.label)}),
        )
        .await
        .value();
    let dup_view_id = case.remember(&dup_view["view"], "id");
    let of_copy = &[("boardId", board), ("viewId", dup_view_id.as_str())];
    case.run("moveBoardView", of_copy, json!({"after": ""}))
        .await;
    case.run("deleteBoardView", of_copy, json!({})).await;
    case.run("restoreBoardView", of_copy, json!({})).await;
    case.run("moveViewCards", of_view, json!({"cardIds": [card]}))
        .await;
    let dup_card = case
        .run(
            "duplicateCard",
            &[("boardId", board), ("cardId", card)],
            json!({}),
        )
        .await
        .value();
    let dup_card_id = case.remember(&dup_card, "id");
    case.run("batchDeleteCards", at, json!({"cardIds": [dup_card_id]}))
        .await;
    case.run("batchRestoreCards", at, json!({"cardIds": [dup_card_id]}))
        .await;
    case.run("batchDeleteCards", at, json!({"cardIds": [dup_card_id]}))
        .await;
}

async fn card_comments(case: &mut Case, board: &str, card: &str) {
    let of_card = &[("boardId", board), ("cardId", card)];
    let said = |text: String| {
        json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": text}]}]})
    };
    let body = said(format!("secret {} card comment", case.label));
    let made = case
        .run("createCardComment", of_card, json!({ "body": body }))
        .await
        .value();
    let comment = case.remember(&made, "id");
    // Answered with an emoji, the card and the comment are read with who answered: the
    // founder alone.
    let reacted_to = |block| {
        [
            ("containerId", board),
            ("blockId", block),
            ("emoji", THUMBS_UP),
        ]
    };
    for block in [card, comment.as_str()] {
        case.run("addReaction", &reacted_to(block), json!({})).await;
    }
    case.present("getCardComments", of_card, &comment).await;
    for block in [card, comment.as_str()] {
        case.run("removeReaction", &reacted_to(block), json!({}))
            .await;
    }
    let of_comment = &[
        ("boardId", board),
        ("cardId", card),
        ("commentId", comment.as_str()),
    ];
    let change =
        json!({"body": said("changed card comment".to_owned()), "readAt": made["updatedAt"]});
    case.run("patchCardComment", of_comment, change).await;
    case.run("getCardCommentVersions", of_comment, json!({}))
        .await;
    case.run("deleteCardComment", of_comment, json!({})).await;
    case.run("restoreCardComment", of_comment, json!({})).await;
}

async fn categories(case: &mut Case, board: &str) {
    let created = case
        .run(
            "createCategory",
            &[],
            json!({
                "name": "Parity category", "userId": case.user, "workspaceId": case.workspace
            }),
        )
        .await
        .value();
    let category = case.remember(&created, "id");
    let at = &[("categoryId", category.as_str())];
    case.run("updateCategory", at, json!({"id": category, "name": "Renamed category", "userId": case.user, "workspaceId": case.workspace})).await;
    case.run(
        "patchCategory",
        at,
        json!({"name": "Patched category", "collapsed": false}),
    )
    .await;
    let board_at = &[("categoryId", category.as_str()), ("boardId", board)];
    case.run("setContainerCategory", board_at, json!({})).await;
    case.run(
        "putCategoryContainer",
        &[("categoryId", category.as_str()), ("containerId", board)],
        json!({"hidden": false}),
    )
    .await;
    case.run("hideContainer", board_at, json!({})).await;
    case.run("unhideContainer", board_at, json!({})).await;
    case.run("reorderCategoryContainers", at, json!([board]))
        .await;
    let listed = case.present("getCategories", &[], &category).await.value();
    let mut order: Vec<_> = listed
        .as_array()
        .expect("categories")
        .iter()
        .map(|entry| entry["id"].clone())
        .collect();
    order.reverse();
    let reordered = case
        .run("reorderCategories", &[], json!(order))
        .await
        .value();
    assert_eq!(reordered, json!(order), "categories are reordered");
    case.run("deleteCategory", at, json!({})).await;
}

/// The founder's stars: one put on the board, listed, put in order and taken off.
async fn stars(case: &mut Case, board: &str) {
    let at = &[("targetId", board)];
    case.run("putFavorite", at, json!({})).await;
    case.present("getFavorites", &[], board).await;
    case.run("reorderFavorites", &[], json!([board])).await;
    case.run("deleteFavorite", at, json!({})).await;
}

async fn people(case: &mut Case, board: &str) -> String {
    let user = case.user.clone();
    let peer = case.peer.clone();
    case.present("getWorkspaceUsers", &[], &peer).await;
    let users = case
        .run("getWorkspaceUsersById", &[], json!([user, peer]))
        .await;
    assert!(
        users.text().contains(&user) && users.text().contains(&peer),
        "requested users are returned"
    );
    case.present("getWorkspaceUser", &[("userId", &peer)], &peer)
        .await;
    case.present("getWorkspaceMembers", &[], &peer).await;
    case.run(
        "setWorkspaceRole",
        &[("user", &peer)],
        json!({"role": "admin"}),
    )
    .await;
    case.run(
        "setWorkspaceRole",
        &[("user", &peer)],
        json!({"role": "member"}),
    )
    .await;
    let at = &[("boardId", board)];
    case.run(
        "addMember",
        at,
        json!({"containerId": board, "userId": peer, "role": "editor"}),
    )
    .await;
    case.present("getMembers", at, &peer).await;
    let member_at = &[("boardId", board), ("userId", peer.as_str())];
    case.run(
        "updateMember",
        member_at,
        json!({"containerId": board, "userId": peer, "role": "viewer"}),
    )
    .await;
    case.run("removeMember", member_at, json!({})).await;
    teams(case, board).await;
    joining(case).await;
    let audited = audit_admission(case).await;
    case.run("setGuestInvitations", &[], json!({"enabled": true}))
        .await;
    let invitation = case
        .run("createInvitation", &[], json!({"role": "member"}))
        .await
        .value();
    let invitation = case.remember(&invitation, "id");
    case.present("getInvitations", &[], &invitation).await;
    case.run(
        "revokeInvitation",
        &[("invitation", &invitation)],
        json!({}),
    )
    .await;
    case.run("removeWorkspaceMember", &[("user", &peer)], json!({}))
        .await;
    audited
}

/// A team of the workspace with the peer in it and a role on the board, renamed and
/// then taken apart; and another that stays.
async fn teams(case: &mut Case, board: &str) {
    let peer = case.peer.clone();
    let made = case
        .run("createTeam", &[], json!({"name": "Design"}))
        .await
        .value();
    let team = case.remember(&made, "id");
    let at = &[("teamId", team.as_str())];
    case.present("getTeams", &[], &team).await;
    case.present("getTeam", at, &team).await;
    case.run("renameTeam", at, json!({"name": "Designers"}))
        .await;
    let member_at = &[("teamId", team.as_str()), ("userId", peer.as_str())];
    case.run("addTeamMember", member_at, json!({})).await;
    let on_board = &[("boardId", board), ("teamId", team.as_str())];
    case.run("setContainerTeam", on_board, json!({"role": "editor"}))
        .await;
    let listed = case
        .present("getContainerTeams", &[("boardId", board)], &team)
        .await
        .text();
    for held in [peer.as_str(), "Designers", "\"editor\""] {
        assert!(
            listed.contains(held),
            "the board's team holds {held}: {listed}"
        );
    }
    case.run("removeContainerTeam", on_board, json!({})).await;
    case.run("removeTeamMember", member_at, json!({})).await;
    case.run("deleteTeam", at, json!({})).await;
    // One team stays, with a role on the board: what the next founder reads of their
    // teams must not be this one.
    let kept = case
        .run("createTeam", &[], json!({"name": "Kept"}))
        .await
        .value();
    let kept = case.remember(&kept, "id");
    case.run(
        "setContainerTeam",
        &[("boardId", board), ("teamId", kept.as_str())],
        json!({"role": "viewer"}),
    )
    .await;
}

async fn joining(case: &mut Case) {
    let made = case
        .run(
            "createBoard",
            &[],
            json!({"title": "Join and leave", "visibility": "open"}),
        )
        .await
        .value();
    let board = case.remember(&made, "id");
    let at = &[("boardId", board.as_str())];
    case.run(
        "addMember",
        at,
        json!({"containerId": board, "userId": case.peer, "role": "admin"}),
    )
    .await;
    case.run("leaveContainer", at, json!({})).await;
    case.run("joinContainer", at, json!({})).await;
}

async fn audit_admission(case: &mut Case) -> String {
    let made = case
        .run(
            "createBoard",
            &[],
            json!({"title": "Audit admission", "visibility": "private"}),
        )
        .await
        .value();
    let board = case.remember(&made, "id");
    let at = &[("boardId", board.as_str())];
    case.run(
        "addMember",
        at,
        json!({"containerId": board, "userId": case.peer, "role": "admin"}),
    )
    .await;
    case.run("leaveContainer", at, json!({})).await;
    // A workspace administrator who cannot read a private board records gaining a role.
    case.run(
        "addMember",
        at,
        json!({"containerId": board, "userId": case.user, "role": "admin"}),
    )
    .await;
    board
}

async fn subscriptions(case: &mut Case, card: &str) {
    let user = case.user.clone();
    case.run(
        "createSubscription",
        &[],
        json!({"blockId": card, "blockType": "card", "userId": user}),
    )
    .await;
    let subscriptions = case
        .run("getSubscriptions", &[("userId", &user)], json!({}))
        .await;
    assert!(
        subscriptions.text().contains(card),
        "the subscription is listed"
    );
    case.run(
        "deleteSubscription",
        &[("userId", &user), ("blockId", card)],
        json!({}),
    )
    .await;
}

async fn templates(case: &mut Case) {
    let made = case
        .run(
            "createBoard",
            &[],
            json!({"title": format!("{} template", case.label), "visibility": "private", "isTemplate": true}),
        )
        .await
        .value();
    let own = case.remember(&made, "id");
    let templates = case.present("getTemplates", &[], &own).await.value();
    let template = templates
        .as_array()
        .expect("templates")
        .iter()
        .find(|entry| entry["id"] != own)
        .expect("a built-in template")["id"]
        .as_str()
        .expect("template ID");
    let copied = case
        .run("duplicateContainer", &[("boardId", template)], json!({}))
        .await
        .value();
    case.remember(&copied["containers"][0], "id");
    let welcomed = case.run("onboard", &[], json!({})).await.value();
    let welcome = case.remember(&welcomed, "boardId");
    let board = case
        .present("getContainer", &[("boardId", &welcome)], &welcome)
        .await
        .value();
    assert_eq!(
        board["workspaceId"], case.workspace,
        "the welcome board is made in this workspace"
    );
}
